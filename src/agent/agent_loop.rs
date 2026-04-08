//! ReAct-style agent loop that keeps one thread-owned turn per incoming request and emits
//! committed messages from the thread-owned message sequence.

use super::{
    feature::AutoCompactor,
    hook::{HookEvent, HookEventKind},
    runtime::AgentRuntime,
    tool::{ToolCallRequest, ToolDefinition},
};
use crate::context::{ChatMessage, ChatMessageRole, Messages};
use crate::{
    compact::{
        CompactManager, CompactProvider, CompactSummary, ContextBudgetEstimator,
        ContextBudgetReport, LLMCompactProvider, MessageCompactionOutcome, StaticCompactProvider,
    },
    config::{AgentCompactConfig, LLMConfig},
    llm::{LLMProvider, LLMRequest},
    model::{IncomingMessage, ReplyTarget},
    thread::{
        Thread, ThreadFinalizedTurn, ThreadToolEvent, ThreadToolEventKind, ThreadTurnEvent,
        ThreadTurnEventKind,
    },
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{error, info};

const TOOL_LOG_PREVIEW_MAX_CHARS: usize = 512;
/// Default max character count used for channel-facing `ToolCall` and `ToolResult` event text.
pub const TOOL_EVENT_PREVIEW_MAX_CHARS: usize = 300;

pub type AgentLoopEvent = ThreadTurnEvent;
pub type AgentLoopEventKind = ThreadTurnEventKind;

#[derive(Debug, Clone)]
pub struct AgentDispatchEvent {
    pub kind: AgentLoopEventKind,
    pub content: String,
    pub metadata: Value,
    pub channel: String,
    pub external_thread_id: Option<String>,
    pub source_message_id: Option<String>,
    pub target: ReplyTarget,
    pub session_id: String,
    pub session_channel: String,
    pub session_user_id: String,
    pub session_external_thread_id: String,
    pub session_thread_id: String,
    pub reply_to_source: bool,
}

/// Bind agent dispatch events to one resolved router/session context.
#[derive(Clone)]
pub struct AgentEventSender {
    channel: String,
    external_thread_id: Option<String>,
    source_message_id: Option<String>,
    target: ReplyTarget,
    session_id: String,
    session_channel: String,
    session_user_id: String,
    session_external_thread_id: String,
    session_thread_id: String,
}

impl AgentEventSender {
    /// Bind one event sender from the current incoming message and resolved thread locator.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentEventSender,
    ///     model::{IncomingMessage, ReplyTarget},
    ///     thread::{ThreadContextLocator, ThreadTurnEvent, ThreadTurnEventKind},
    /// };
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_1".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: Some("thread_ext".to_string()),
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    /// let locator = ThreadContextLocator::new(
    ///     Some("session_1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "thread_internal",
    /// );
    ///
    /// let sender = AgentEventSender::from_incoming_and_locator(&incoming, &locator);
    /// let dispatch = sender.prepare_dispatch_event(ThreadTurnEvent {
    ///     kind: ThreadTurnEventKind::TextOutput,
    ///     content: "done".to_string(),
    ///     metadata: json!({}),
    /// }, true);
    /// assert_eq!(dispatch.session_thread_id, "thread_internal");
    /// ```
    pub fn from_incoming_and_locator(
        incoming: &IncomingMessage,
        locator: &crate::thread::ThreadContextLocator,
    ) -> Self {
        Self {
            channel: incoming.channel.clone(),
            external_thread_id: incoming.external_thread_id.clone(),
            source_message_id: incoming.external_message_id.clone(),
            target: incoming.reply_target.clone(),
            session_id: locator.session_id.clone().unwrap_or_default(),
            session_channel: locator.channel.clone(),
            session_user_id: locator.user_id.clone(),
            session_external_thread_id: locator.external_thread_id.clone(),
            session_thread_id: locator.thread_id.clone(),
        }
    }

    /// Materialize one committed message event into a router-ready dispatch payload.
    pub fn prepare_dispatch_event(
        &self,
        event: AgentLoopEvent,
        reply_to_source: bool,
    ) -> AgentDispatchEvent {
        AgentDispatchEvent {
            kind: event.kind,
            content: event.content,
            metadata: event.metadata,
            channel: self.channel.clone(),
            external_thread_id: self.external_thread_id.clone(),
            source_message_id: self.source_message_id.clone(),
            target: self.target.clone(),
            session_id: self.session_id.clone(),
            session_channel: self.session_channel.clone(),
            session_user_id: self.session_user_id.clone(),
            session_external_thread_id: self.session_external_thread_id.clone(),
            session_thread_id: self.session_thread_id.clone(),
            reply_to_source,
        }
    }
}

pub struct AgentLoopOutput {
    pub reply: String,
    pub metadata: Value,
    pub turns: Vec<CompletedAgentTurn>,
}

#[derive(Debug, Clone)]
pub struct CompletedAgentTurn {
    pub turn: ThreadFinalizedTurn,
}

#[async_trait]
pub trait AgentCommittedMessageHandler: Send {
    async fn on_committed_message(
        &mut self,
        turn_id: uuid::Uuid,
        thread_context: &mut Thread,
        message: ChatMessage,
        dispatch_events: Vec<AgentDispatchEvent>,
    ) -> Result<()>;
}

struct NoopCommittedMessageHandler;

#[async_trait]
impl AgentCommittedMessageHandler for NoopCommittedMessageHandler {
    async fn on_committed_message(
        &mut self,
        _turn_id: uuid::Uuid,
        _thread_context: &mut Thread,
        _message: ChatMessage,
        _dispatch_events: Vec<AgentDispatchEvent>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Integration-test probe state captured at one agent-loop iteration boundary.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTLoopState {
    pub iteration: usize,
    pub request_messages: Vec<ChatMessage>,
    pub turn_events: Vec<AgentLoopEvent>,
}

/// Integration-test snapshot captured after request preparation.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTRequestSnapshot {
    pub iteration: usize,
    pub messages: Messages,
    pub tools: Vec<ToolDefinition>,
    pub budget_report: ContextBudgetReport,
}

/// Integration-test snapshot captured after one LLM response returns.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTLLMResponseSnapshot {
    pub iteration: usize,
    pub message: Option<ChatMessage>,
    pub tool_calls: Vec<crate::llm::LLMToolCall>,
}

/// Integration-test snapshot captured before one tool execution starts.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTToolCallSnapshot {
    pub iteration: usize,
    pub tool_call_id: String,
    pub request: ToolCallRequest,
}

/// Integration-test snapshot captured after one tool execution completes.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTToolResultSnapshot {
    pub iteration: usize,
    pub tool_call_id: String,
    pub request: ToolCallRequest,
    pub result: super::ToolCallResult,
}

/// Integration-test snapshot captured after one compact action is handled.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AgentLoopUTCompactSnapshot {
    pub iteration: usize,
    pub reason: String,
    pub requested_by_model: bool,
    pub is_error: bool,
    pub budget_report: ContextBudgetReport,
    pub outcome: Option<MessageCompactionOutcome>,
    pub error: Option<String>,
    pub request_messages: Vec<ChatMessage>,
    pub turn_events: Vec<AgentLoopEvent>,
}

/// Integration-test probe hooks for observing intermediate agent-loop state.
#[doc(hidden)]
pub trait AgentLoopUTProber: Send {
    fn on_loop_begin(&mut self, _state: &AgentLoopUTLoopState) {}

    fn on_request_prepared(&mut self, _snapshot: &AgentLoopUTRequestSnapshot) {}

    fn on_llm_response(&mut self, _snapshot: &AgentLoopUTLLMResponseSnapshot) {}

    fn on_tool_call_start(&mut self, _snapshot: &AgentLoopUTToolCallSnapshot) {}

    fn on_tool_result(&mut self, _snapshot: &AgentLoopUTToolResultSnapshot) {}

    fn on_compact(&mut self, _snapshot: &AgentLoopUTCompactSnapshot) {}

    fn on_loop_end(&mut self, _state: &AgentLoopUTLoopState) {}
}

/// Alias used by integration tests when passing one mutable loop probe.
#[doc(hidden)]
pub type UTProbe<'a> = &'a mut dyn AgentLoopUTProber;

struct AgentLoopUTProberHandle<'a> {
    probe: Option<UTProbe<'a>>,
}

impl<'a> AgentLoopUTProberHandle<'a> {
    fn new(probe: Option<UTProbe<'a>>) -> Self {
        Self { probe }
    }

    fn build_loop_state(&self, iteration: usize, thread_context: &Thread) -> AgentLoopUTLoopState {
        AgentLoopUTLoopState {
            iteration,
            request_messages: thread_context.messages(),
            turn_events: thread_context.current_turn_events(),
        }
    }

    fn on_loop_begin(&mut self, iteration: usize, thread_context: &Thread) {
        let state = self.build_loop_state(iteration, thread_context);
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_loop_begin(&state);
        }
    }

    fn on_request_prepared(&mut self, iteration: usize, request_state: &RequestState) {
        let snapshot = AgentLoopUTRequestSnapshot {
            iteration,
            messages: request_state.messages.clone(),
            tools: request_state.tools.clone(),
            budget_report: request_state.budget_report.clone(),
        };
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_request_prepared(&snapshot);
        }
    }

    fn on_llm_response(&mut self, iteration: usize, response: &crate::llm::LLMResponse) {
        let snapshot = AgentLoopUTLLMResponseSnapshot {
            iteration,
            message: response.message.clone(),
            tool_calls: response.tool_calls.clone(),
        };
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_llm_response(&snapshot);
        }
    }

    fn on_tool_call_start(&mut self, snapshot: AgentLoopUTToolCallSnapshot) {
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_tool_call_start(&snapshot);
        }
    }

    fn on_tool_result(&mut self, snapshot: AgentLoopUTToolResultSnapshot) {
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_tool_result(&snapshot);
        }
    }

    fn on_compact(&mut self, snapshot: AgentLoopUTCompactSnapshot) {
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_compact(&snapshot);
        }
    }

    fn on_loop_end(&mut self, iteration: usize, thread_context: &Thread) {
        let state = self.build_loop_state(iteration, thread_context);
        if let Some(probe) = self.probe.as_deref_mut() {
            probe.on_loop_end(&state);
        }
    }
}

pub struct AgentLoop {
    llm: Arc<dyn LLMProvider>,
    runtime: AgentRuntime,
    compact_config: AgentCompactConfig,
    budget_estimator: ContextBudgetEstimator,
    compact_manager: CompactManager,
    auto_compactor: AutoCompactor,
}

impl AgentLoop {
    /// Create an agent loop bound to one LLM provider and runtime container.
    pub fn new(llm: Arc<dyn LLMProvider>, runtime: AgentRuntime) -> Self {
        Self::with_compact_config(
            llm,
            runtime,
            LLMConfig::default(),
            AgentCompactConfig::default(),
        )
    }

    /// Create an agent loop with explicit compact and budget config.
    pub fn with_compact_config(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
    ) -> Self {
        let compact_provider = build_compact_provider(&llm, &compact_config);
        Self::with_compact_provider(llm, runtime, llm_config, compact_config, compact_provider)
    }

    /// Create an agent loop with an explicitly injected compact provider.
    pub fn with_compact_provider(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
        compact_provider: Arc<dyn CompactProvider>,
    ) -> Self {
        let budget_estimator = ContextBudgetEstimator::from_config(&llm_config, &compact_config);
        let compact_manager = CompactManager::new(compact_provider);
        let auto_compactor = AutoCompactor::new(compact_config.clone());

        Self {
            llm,
            runtime,
            compact_config,
            budget_estimator,
            compact_manager,
            auto_compactor,
        }
    }

    /// Return the runtime used by this loop.
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }

    /// Run one agent request from `Thread + incoming` and return every finalized turn produced by
    /// the internal loop.
    pub async fn run_v1(
        &self,
        event_tx: AgentEventSender,
        incoming: &IncomingMessage,
        thread_context: Thread,
    ) -> Result<AgentLoopOutput> {
        let mut thread_context = thread_context;
        let mut ut_probe = AgentLoopUTProberHandle::new(None);
        let mut on_committed_message = NoopCommittedMessageHandler;
        self.run_live_thread(
            event_tx,
            &mut thread_context,
            incoming_message(incoming),
            incoming.external_message_id.clone(),
            &mut ut_probe,
            &mut on_committed_message,
        )
        .await
    }

    /// Run one agent loop while exposing doc-hidden UT probe hooks for integration tests.
    #[doc(hidden)]
    pub async fn run_v1_with_ut_probe(
        &self,
        event_tx: AgentEventSender,
        incoming: &IncomingMessage,
        thread_context: Thread,
        ut_probe: Option<UTProbe<'_>>,
    ) -> Result<AgentLoopOutput> {
        let mut ut_probe = AgentLoopUTProberHandle::new(ut_probe);
        let mut thread_context = thread_context;
        let mut on_committed_message = NoopCommittedMessageHandler;
        self.run_live_thread(
            event_tx,
            &mut thread_context,
            incoming_message(incoming),
            incoming.external_message_id.clone(),
            &mut ut_probe,
            &mut on_committed_message,
        )
        .await
    }

    /// Run one live thread owned by the caller and invoke the commit hook for every committed
    /// message.
    pub async fn run_locked_thread<H>(
        &self,
        event_tx: AgentEventSender,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
        on_committed_message: &mut H,
    ) -> Result<AgentLoopOutput>
    where
        H: AgentCommittedMessageHandler,
    {
        let mut ut_probe = AgentLoopUTProberHandle::new(None);
        self.run_live_thread(
            event_tx,
            thread_context,
            incoming_message(incoming),
            incoming.external_message_id.clone(),
            &mut ut_probe,
            on_committed_message,
        )
        .await
    }

    async fn run_live_thread<H>(
        &self,
        event_tx: AgentEventSender,
        mut thread_context: &mut Thread,
        initial_message: ChatMessage,
        external_message_id: Option<String>,
        ut_probe: &mut AgentLoopUTProberHandle<'_>,
        on_committed_message: &mut H,
    ) -> Result<AgentLoopOutput>
    where
        H: AgentCommittedMessageHandler,
    {
        self.prepare_thread_runtime(&mut thread_context).await?;

        let hooks = self.runtime.hooks();
        let thread_locator = thread_context.locator.clone();
        let thread_id = thread_locator.thread_id.clone();
        hooks
            .emit(HookEvent {
                kind: HookEventKind::UserPromptSubmit,
                payload: json!({
                    "channel": thread_locator.channel.clone(),
                    "user_id": thread_locator.user_id.clone(),
                    "thread_id": thread_id,
                }),
            })
            .await?;

        let mut used_tool_names = Vec::new();
        let mut last_visible_tools = Vec::new();
        let mut last_budget_report = None;
        let mut loop_iteration = 0usize;
        let mut reply_to_source = true;
        thread_context.begin_turn(external_message_id, initial_message.created_at)?;
        commit_message(
            &event_tx,
            &mut thread_context,
            &mut reply_to_source,
            initial_message,
            Vec::new(),
            on_committed_message,
        )
        .await?;

        let turn_completion = loop {
            ut_probe.on_loop_begin(loop_iteration, &thread_context);
            let pre_turn_request_state = self.prepare_request_state(&thread_context).await?;
            if self.should_runtime_compact(&thread_context, &pre_turn_request_state.budget_report) {
                if let Some(outcome) = self
                    .execute_turn_compaction(
                        &hooks,
                        &thread_id,
                        &mut thread_context,
                        "runtime_threshold",
                        false,
                        None,
                        &pre_turn_request_state.budget_report,
                    )
                    .await?
                {
                    ut_probe.on_compact(AgentLoopUTCompactSnapshot {
                        iteration: loop_iteration,
                        reason: "runtime_threshold".to_string(),
                        requested_by_model: false,
                        is_error: false,
                        budget_report: pre_turn_request_state.budget_report.clone(),
                        outcome: Some(outcome),
                        error: None,
                        request_messages: thread_context.messages(),
                        turn_events: thread_context.current_turn_events(),
                    });
                }
            }

            let turn_result = async {
                let request_state = self.prepare_request_state(&thread_context).await?;
                ut_probe.on_request_prepared(loop_iteration, &request_state);
                last_visible_tools = request_state.tools.clone();
                last_budget_report = Some(request_state.budget_report.clone());

                let RequestState {
                    messages,
                    tools,
                    budget_report,
                } = request_state;

                info!(
                    last_content = %messages
                        .last()
                        .map(|m| m.content.chars().take(50).collect::<String>())
                        .unwrap_or("None".into()),
                    "[LLM-GENERATE] before",
                );

                let response = self.llm.generate(LLMRequest { messages, tools }).await?;
                ut_probe.on_llm_response(loop_iteration, &response);

                info!(
                    toolcalls_count = %response.tool_calls.len(),
                    content = response
                        .message
                        .as_ref()
                        .map(|m| m.content.chars().take(50).collect::<String>())
                        .unwrap_or("None".into()),
                    "[LLM-GENERATE] after"
                );

                if response.tool_calls.is_empty() {
                    let assistant_message = response
                        .message
                        .context("llm response did not contain assistant text or tool calls")?;
                    if assistant_message.content.trim().is_empty() {
                        bail!("llm final response did not contain assistant text");
                    }

                    commit_message(
                        &event_tx,
                        &mut thread_context,
                        &mut reply_to_source,
                        assistant_message.clone(),
                        vec![AgentLoopEvent {
                            kind: AgentLoopEventKind::TextOutput,
                            content: assistant_message.content.clone(),
                            metadata: json!({
                                "source": "llm_response",
                                "is_final": true,
                            }),
                        }],
                        on_committed_message,
                    )
                    .await?;
                    return Ok::<TurnLoopSuccess, anyhow::Error>(TurnLoopSuccess {
                        reply: assistant_message.content,
                        completed: true,
                    });
                }

                let turn_reply = response
                    .message
                    .as_ref()
                    .map(|message| message.content.clone())
                    .unwrap_or_default();
                if let Some(message) = response.message.as_ref()
                    && !message.content.trim().is_empty()
                {
                    commit_message(
                        &event_tx,
                        &mut thread_context,
                        &mut reply_to_source,
                        message.clone(),
                        vec![AgentLoopEvent {
                            kind: AgentLoopEventKind::TextOutput,
                            content: message.content.clone(),
                            metadata: json!({
                                "source": "llm_response",
                                "is_final": false,
                            }),
                        }],
                        on_committed_message,
                    )
                    .await?;
                }
                let tool_call_message_created_at = response
                    .message
                    .as_ref()
                    .map(|message| message.created_at)
                    .unwrap_or_else(Utc::now);
                for provider_tool_call in &response.tool_calls {
                    let tool_call = ToolCallRequest {
                        name: provider_tool_call.name.clone(),
                        arguments: provider_tool_call.arguments.clone(),
                    };
                    commit_message(
                        &event_tx,
                        &mut thread_context,
                        &mut reply_to_source,
                        build_tool_call_message(provider_tool_call, tool_call_message_created_at),
                        vec![build_tool_call_event(&tool_call, &provider_tool_call.id)],
                        on_committed_message,
                    )
                    .await?;
                }

                for provider_tool_call in response.tool_calls {
                    let tool_call = ToolCallRequest {
                        name: provider_tool_call.name.clone(),
                        arguments: provider_tool_call.arguments.clone(),
                    };
                    used_tool_names.push(tool_call.name.clone());
                    hooks
                        .emit(HookEvent {
                            kind: HookEventKind::PreToolUse,
                            payload: json!({
                                "tool": tool_call.name.clone(),
                                "arguments": tool_call.arguments.clone(),
                                "tool_call_id": provider_tool_call.id.clone(),
                            }),
                        })
                        .await?;
                    ut_probe.on_tool_call_start(AgentLoopUTToolCallSnapshot {
                        iteration: loop_iteration,
                        tool_call_id: provider_tool_call.id.clone(),
                        request: tool_call.clone(),
                    });
                    info!(
                        thread_id = %thread_id,
                        tool_name = %tool_call.name,
                        tool_call_id = %provider_tool_call.id,
                        arguments_preview = %truncate_tool_log_preview(
                            &tool_call.arguments.to_string(),
                            TOOL_LOG_PREVIEW_MAX_CHARS,
                        ),
                        "agent loop starting tool call"
                    );

                    let tool_result = if tool_call.name == "compact" {
                        self.handle_model_requested_compact(
                            &hooks,
                            &thread_id,
                            &mut thread_context,
                            &tool_call,
                            &provider_tool_call.id,
                            &budget_report,
                            ut_probe,
                            loop_iteration,
                        )
                        .await?
                    } else {
                        let tool_result =
                            match self.call_thread_tool(&mut thread_context, &tool_call).await {
                                Ok(result) => {
                                    hooks
                                        .emit(HookEvent {
                                            kind: HookEventKind::PostToolUse,
                                            payload: json!({
                                                "tool": tool_call.name.clone(),
                                                "result": result.metadata.clone(),
                                            }),
                                        })
                                        .await?;
                                    result
                                }
                                Err(error) => {
                                    hooks
                                        .emit(HookEvent {
                                            kind: HookEventKind::PostToolUseFailure,
                                            payload: json!({
                                                "tool": tool_call.name.clone(),
                                                "error": error.to_string(),
                                            }),
                                        })
                                        .await?;
                                    super::ToolCallResult {
                                        content: error.to_string(),
                                        metadata: json!({
                                            "tool": tool_call.name.clone(),
                                        }),
                                        is_error: true,
                                    }
                                }
                            };
                        thread_context.record_tool_event(build_thread_tool_event(
                            &tool_call,
                            &provider_tool_call.id,
                            &tool_result,
                        ));
                        info!(
                            thread_id = %thread_id,
                            tool_name = %tool_call.name,
                            tool_call_id = %provider_tool_call.id,
                            is_error = tool_result.is_error,
                            result_preview = %truncate_tool_log_preview(
                                &tool_result.content,
                                TOOL_LOG_PREVIEW_MAX_CHARS,
                            ),
                            "agent loop completed tool call"
                        );

                        tool_result
                    };

                    let tool_result_message = ChatMessage::new(
                        ChatMessageRole::ToolResult,
                        format_tool_result_content(&tool_result.content, tool_result.is_error),
                        Utc::now(),
                    )
                    .with_tool_call_id(provider_tool_call.id.clone());
                    commit_message(
                        &event_tx,
                        &mut thread_context,
                        &mut reply_to_source,
                        tool_result_message,
                        vec![build_tool_result_event(
                            &tool_call,
                            &provider_tool_call.id,
                            &tool_result,
                        )],
                        on_committed_message,
                    )
                    .await?;
                    ut_probe.on_tool_result(AgentLoopUTToolResultSnapshot {
                        iteration: loop_iteration,
                        tool_call_id: provider_tool_call.id,
                        request: tool_call,
                        result: tool_result,
                    });
                }

                Ok::<TurnLoopSuccess, anyhow::Error>(TurnLoopSuccess {
                    reply: turn_reply,
                    completed: false,
                })
            }
            .await;

            let completed_at = Utc::now();
            match turn_result {
                Ok(turn_result) => {
                    ut_probe.on_loop_end(loop_iteration, &thread_context);
                    if turn_result.completed {
                        break TurnCompletion {
                            reply: turn_result.reply,
                            failure_error: None,
                        };
                    }
                }
                Err(error) => {
                    let error_message = format!("{error:#}");
                    error!(
                        thread_id = %thread_context.locator.thread_id,
                        external_thread_id = %thread_context.locator.external_thread_id,
                        error = %error_message,
                        "agent loop encountered one unexpected turn failure"
                    );
                    ut_probe.on_loop_end(loop_iteration, &thread_context);
                    let failure_reply = format!("[openjarvis][agent_error] {error_message}");
                    let failure_message = ChatMessage::new(
                        ChatMessageRole::Assistant,
                        failure_reply.clone(),
                        completed_at,
                    );
                    commit_message(
                        &event_tx,
                        &mut thread_context,
                        &mut reply_to_source,
                        failure_message,
                        vec![AgentLoopEvent {
                            kind: AgentLoopEventKind::TextOutput,
                            content: failure_reply.clone(),
                            metadata: json!({
                                "source": "turn_failure",
                                "is_final": true,
                                "is_error": true,
                            }),
                        }],
                        on_committed_message,
                    )
                    .await?;
                    break TurnCompletion {
                        reply: failure_reply,
                        failure_error: Some(error_message),
                    };
                }
            }

            loop_iteration += 1;
        };

        let completed_at = Utc::now();
        let turn = if let Some(error_message) = turn_completion.failure_error.clone() {
            thread_context.finalize_turn_failure(error_message, completed_at)?
        } else {
            thread_context.finalize_turn_success(turn_completion.reply.clone(), completed_at)?
        };
        let completed_turns = vec![finalize_completed_turn(turn)];
        let last_turn = completed_turns
            .last()
            .context("agent loop did not finalize any turn")?;
        let metadata = build_loop_output_metadata(
            &self.runtime,
            &last_turn.turn,
            &used_tool_names,
            &last_visible_tools,
            last_budget_report.clone(),
        )
        .await;
        hooks
            .emit(HookEvent {
                kind: HookEventKind::Notification,
                payload: json!({
                    "reply_preview": turn_completion.reply.clone(),
                    "runtime": metadata,
                }),
            })
            .await?;

        Ok(AgentLoopOutput {
            reply: turn_completion.reply,
            metadata,
            turns: completed_turns,
        })
    }

    fn auto_compact_enabled_for_thread(&self, thread_context: &Thread) -> bool {
        self.compact_config.enabled()
            && thread_context.auto_compact_enabled(self.compact_config.auto_compact())
    }

    async fn prepare_thread_runtime(&self, thread_context: &mut Thread) -> Result<()> {
        if thread_context.has_runtime() {
            return Ok(());
        }
        info!(
            thread_id = %thread_context.locator.thread_id,
            "preparing thread runtime"
        );
        self.runtime.tools().register_builtin_tools().await?;
        Ok(())
    }

    async fn prepare_request_state(&self, thread_context: &Thread) -> Result<RequestState> {
        let base_tools = if thread_context.has_runtime() {
            thread_context.visible_tools(false).await?
        } else {
            self.runtime.list_tools(thread_context, false).await?
        };
        let messages = thread_context.messages();
        let base_budget_report = self.budget_estimator.estimate(&messages, &base_tools);
        let compact_visible = self.auto_compact_enabled_for_thread(thread_context)
            && self
                .auto_compactor
                .compact_tool_visible(&base_budget_report);
        let tools = if compact_visible {
            if thread_context.has_runtime() {
                thread_context.visible_tools(true).await?
            } else {
                self.runtime.list_tools(thread_context, true).await?
            }
        } else {
            base_tools
        };
        let budget_report = if compact_visible {
            self.budget_estimator.estimate(&messages, &tools)
        } else {
            base_budget_report
        };
        info!(
            thread_id = %thread_context.locator.thread_id,
            total_estimated_tokens = budget_report.total_estimated_tokens,
            utilization_ratio = budget_report.utilization_ratio,
            tool_count = tools.len(),
            compact_visible,
            "prepared thread-owned request budget"
        );
        Ok(RequestState {
            messages,
            tools,
            budget_report,
        })
    }

    fn should_runtime_compact(
        &self,
        thread_context: &Thread,
        budget_report: &ContextBudgetReport,
    ) -> bool {
        self.compact_config.enabled()
            && thread_context
                .messages()
                .iter()
                .any(|message| message.role != ChatMessageRole::System)
            && self
                .auto_compactor
                .runtime_compaction_required(budget_report)
    }

    async fn call_thread_tool(
        &self,
        thread_context: &mut Thread,
        tool_call: &ToolCallRequest,
    ) -> Result<super::ToolCallResult> {
        if thread_context.has_runtime() {
            thread_context.call_tool(tool_call.clone()).await
        } else {
            self.runtime
                .call_tool(thread_context, tool_call.clone())
                .await
        }
    }

    async fn execute_turn_compaction(
        &self,
        hooks: &Arc<super::HookRegistry>,
        thread_id: &str,
        thread_context: &mut Thread,
        reason: &str,
        requested_by_model: bool,
        tool_call_id: Option<&str>,
        budget_report: &ContextBudgetReport,
    ) -> Result<Option<MessageCompactionOutcome>> {
        let compactable_messages = thread_context.messages();
        if compactable_messages.is_empty() {
            let compact_event = build_compact_event(
                reason,
                requested_by_model,
                false,
                budget_report,
                None,
                tool_call_id,
                None,
            );
            thread_context.buffer_turn_event(compact_event)?;
            return Ok(None);
        }

        hooks
            .emit(HookEvent {
                kind: HookEventKind::PreCompact,
                payload: json!({
                    "thread_id": thread_id,
                    "reason": reason,
                    "budget_report": budget_report,
                    "active_message_count": compactable_messages.len(),
                }),
            })
            .await?;
        info!(
            thread_id,
            reason,
            active_message_count = compactable_messages.len(),
            total_estimated_tokens = budget_report.total_estimated_tokens,
            utilization_ratio = budget_report.utilization_ratio,
            "triggering thread compact"
        );

        let Some(outcome) = self
            .compact_manager
            .compact_messages(&compactable_messages, Utc::now())
            .await?
        else {
            return Ok(None);
        };
        thread_context.replace_messages_after_compaction(outcome.compacted_messages.clone())?;
        thread_context.buffer_turn_event(build_compact_event(
            reason,
            requested_by_model,
            false,
            budget_report,
            Some(&outcome),
            tool_call_id,
            None,
        ))?;
        info!(
            thread_id,
            reason,
            compacted_message_count = thread_context.messages().len(),
            "thread compact completed"
        );
        Ok(Some(outcome))
    }

    async fn handle_model_requested_compact(
        &self,
        hooks: &Arc<super::HookRegistry>,
        thread_id: &str,
        thread_context: &mut Thread,
        tool_call: &ToolCallRequest,
        tool_call_id: &str,
        budget_report: &ContextBudgetReport,
        ut_probe: &mut AgentLoopUTProberHandle<'_>,
        loop_iteration: usize,
    ) -> Result<super::ToolCallResult> {
        if !self.compact_config.enabled() {
            let error_message = "compact runtime is disabled".to_string();
            hooks
                .emit(HookEvent {
                    kind: HookEventKind::PostToolUseFailure,
                    payload: json!({
                        "tool": tool_call.name.clone(),
                        "error": error_message.clone(),
                    }),
                })
                .await?;
            thread_context.record_tool_event(build_compact_thread_tool_event(
                tool_call,
                tool_call_id,
                build_compact_event(
                    "tool_requested",
                    true,
                    true,
                    budget_report,
                    None,
                    Some(tool_call_id),
                    Some(&error_message),
                )
                .metadata
                .clone(),
                true,
            ));
            thread_context.buffer_turn_event(build_compact_event(
                "tool_requested",
                true,
                true,
                budget_report,
                None,
                Some(tool_call_id),
                Some(&error_message),
            ))?;
            let result = super::ToolCallResult {
                content: error_message.clone(),
                metadata: json!({
                    "event_kind": "compact",
                    "tool_call_id": tool_call_id,
                }),
                is_error: true,
            };
            ut_probe.on_compact(AgentLoopUTCompactSnapshot {
                iteration: loop_iteration,
                reason: "tool_requested".to_string(),
                requested_by_model: true,
                is_error: true,
                budget_report: budget_report.clone(),
                outcome: None,
                error: Some(error_message),
                request_messages: thread_context.messages(),
                turn_events: thread_context.current_turn_events(),
            });
            return Ok(result);
        }

        match self
            .execute_turn_compaction(
                hooks,
                thread_id,
                thread_context,
                "tool_requested",
                true,
                Some(tool_call_id),
                budget_report,
            )
            .await
        {
            Ok(outcome) => {
                hooks
                    .emit(HookEvent {
                        kind: HookEventKind::PostToolUse,
                        payload: json!({
                            "tool": tool_call.name.clone(),
                            "result": thread_context.current_turn_events().last().map(|event| event.metadata.clone()),
                        }),
                    })
                    .await?;
                thread_context.record_tool_event(build_compact_thread_tool_event(
                    tool_call,
                    tool_call_id,
                    thread_context
                        .current_turn_events()
                        .last()
                        .map(|event| event.metadata.clone())
                        .unwrap_or_else(default_compact_event_metadata),
                    false,
                ));
                ut_probe.on_compact(AgentLoopUTCompactSnapshot {
                    iteration: loop_iteration,
                    reason: "tool_requested".to_string(),
                    requested_by_model: true,
                    is_error: false,
                    budget_report: budget_report.clone(),
                    outcome: outcome.clone(),
                    error: None,
                    request_messages: thread_context.messages(),
                    turn_events: thread_context.current_turn_events(),
                });
                let content = outcome
                    .as_ref()
                    .map(|value| {
                        format!(
                            "compact completed: compacted {} messages from current chat history",
                            value.source_message_count
                        )
                    })
                    .unwrap_or_else(|| {
                        "compact skipped: no chat history was available to compact".to_string()
                    });
                Ok(super::ToolCallResult {
                    content,
                    metadata: json!({
                        "event_kind": "compact",
                        "tool_call_id": tool_call_id,
                    }),
                    is_error: false,
                })
            }
            Err(error) => {
                let error_message = error.to_string();
                hooks
                    .emit(HookEvent {
                        kind: HookEventKind::PostToolUseFailure,
                        payload: json!({
                            "tool": tool_call.name.clone(),
                            "error": error_message.clone(),
                        }),
                    })
                    .await?;
                thread_context.record_tool_event(build_compact_thread_tool_event(
                    tool_call,
                    tool_call_id,
                    build_compact_event(
                        "tool_requested",
                        true,
                        true,
                        budget_report,
                        None,
                        Some(tool_call_id),
                        Some(&error_message),
                    )
                    .metadata
                    .clone(),
                    true,
                ));
                thread_context.buffer_turn_event(build_compact_event(
                    "tool_requested",
                    true,
                    true,
                    budget_report,
                    None,
                    Some(tool_call_id),
                    Some(&error_message),
                ))?;
                ut_probe.on_compact(AgentLoopUTCompactSnapshot {
                    iteration: loop_iteration,
                    reason: "tool_requested".to_string(),
                    requested_by_model: true,
                    is_error: true,
                    budget_report: budget_report.clone(),
                    outcome: None,
                    error: Some(error_message.clone()),
                    request_messages: thread_context.messages(),
                    turn_events: thread_context.current_turn_events(),
                });
                Ok(super::ToolCallResult {
                    content: error_message,
                    metadata: json!({
                        "event_kind": "compact",
                        "tool_call_id": tool_call_id,
                    }),
                    is_error: true,
                })
            }
        }
    }
}

struct RequestState {
    messages: Messages,
    tools: Vec<ToolDefinition>,
    budget_report: ContextBudgetReport,
}

struct TurnLoopSuccess {
    reply: String,
    completed: bool,
}

struct TurnCompletion {
    reply: String,
    failure_error: Option<String>,
}

fn build_tool_call_event(tool_call: &ToolCallRequest, tool_call_id: &str) -> AgentLoopEvent {
    let tool_call_arguments = tool_call.arguments.to_string();
    let truncated_tool_call_arguments =
        truncate_tool_message(&tool_call_arguments, TOOL_EVENT_PREVIEW_MAX_CHARS);
    AgentLoopEvent {
        kind: AgentLoopEventKind::ToolCall,
        content: format!(
            "[openjarvis][tool_call] {} {}",
            tool_call.name, truncated_tool_call_arguments
        ),
        metadata: json!({
            "tool": tool_call.name,
            "arguments": tool_call.arguments,
            "tool_call_id": tool_call_id,
        }),
    }
}

fn build_tool_result_event(
    tool_call: &ToolCallRequest,
    tool_call_id: &str,
    tool_result: &super::ToolCallResult,
) -> AgentLoopEvent {
    let tool_result_content =
        format_tool_result_content(&tool_result.content, tool_result.is_error);
    let truncated_tool_result_content =
        truncate_tool_message(&tool_result_content, TOOL_EVENT_PREVIEW_MAX_CHARS);
    AgentLoopEvent {
        kind: AgentLoopEventKind::ToolResult,
        content: format!(
            "[openjarvis][tool_result] {}",
            truncated_tool_result_content
        ),
        metadata: json!({
            "tool": tool_call.name.clone(),
            "is_error": tool_result.is_error,
            "metadata": tool_result.metadata,
            "tool_call_id": tool_call_id,
        }),
    }
}

fn finalize_completed_turn(turn: ThreadFinalizedTurn) -> CompletedAgentTurn {
    CompletedAgentTurn { turn }
}

async fn commit_message<H>(
    event_tx: &AgentEventSender,
    thread_context: &mut Thread,
    reply_to_source: &mut bool,
    message: ChatMessage,
    turn_events: Vec<AgentLoopEvent>,
    on_committed_message: &mut H,
) -> Result<()>
where
    H: AgentCommittedMessageHandler,
{
    thread_context.push_message(message.clone())?;
    for event in &turn_events {
        thread_context.buffer_turn_event(event.clone())?;
    }
    let dispatch_events = turn_events
        .into_iter()
        .map(|event| {
            let dispatch = event_tx.prepare_dispatch_event(event, *reply_to_source);
            *reply_to_source = false;
            dispatch
        })
        .collect::<Vec<_>>();
    let turn_id = thread_context
        .current_turn_id()
        .context("thread did not expose one active turn id for committed message")?;
    on_committed_message
        .on_committed_message(turn_id, thread_context, message, dispatch_events)
        .await?;
    Ok(())
}

async fn build_loop_output_metadata(
    runtime: &AgentRuntime,
    turn: &ThreadFinalizedTurn,
    used_tool_names: &[String],
    last_visible_tools: &[ToolDefinition],
    last_budget_report: Option<ContextBudgetReport>,
) -> Value {
    let mcp_server_count = runtime.tools().mcp().list_servers().await.len();
    let hook_handler_count = runtime.hooks().len().await;
    json!({
        "tool_count": last_visible_tools.len(),
        "mcp_server_count": mcp_server_count,
        "hook_handler_count": hook_handler_count,
        "used_tool_name": used_tool_names.first().cloned(),
        "used_tool_names": used_tool_names,
        "loaded_toolsets": turn.snapshot.load_toolsets(),
        "event_count": turn.events.len(),
        "context_budget": last_budget_report,
        "turn_status": turn.status,
        "turn_id": turn.turn_id,
    })
}

fn build_tool_call_message(
    tool_call: &crate::llm::LLMToolCall,
    created_at: chrono::DateTime<Utc>,
) -> ChatMessage {
    ChatMessage::new(ChatMessageRole::Toolcall, "", created_at)
        .with_tool_calls(vec![tool_call.clone()])
}

fn format_tool_result_content(content: &str, is_error: bool) -> String {
    if is_error {
        return format!("Tool execution failed: {content}");
    }

    content.to_string()
}

/// Truncate channel-facing tool event content without affecting the full tool history kept for
/// subsequent model turns.
#[doc(hidden)]
pub fn truncate_tool_message(content: &str, max_chars: usize) -> String {
    truncate_text_with_total_chars(content, max_chars)
}

#[doc(hidden)]
pub fn truncate_tool_log_preview(content: &str, max_chars: usize) -> String {
    truncate_text_with_total_chars(content, max_chars)
}

fn truncate_text_with_total_chars(content: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return format!("...(truncated, total_chars={})", content.chars().count());
    }

    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }

    let truncated = content.chars().take(max_chars).collect::<String>();
    format!("{truncated}...(truncated, total_chars={char_count})")
}

fn build_compact_event(
    reason: &str,
    requested_by_model: bool,
    is_error: bool,
    budget_report: &ContextBudgetReport,
    outcome: Option<&MessageCompactionOutcome>,
    tool_call_id: Option<&str>,
    error: Option<&str>,
) -> AgentLoopEvent {
    let compacted = outcome.is_some() && !is_error;
    let content = if let Some(error) = error {
        format!("[openjarvis][compact] failed: {error}")
    } else if let Some(outcome) = outcome {
        format!(
            "[openjarvis][compact] compacted {} messages from current chat history",
            outcome.source_message_count
        )
    } else {
        "[openjarvis][compact] no chat history was available to compact".to_string()
    };

    AgentLoopEvent {
        kind: AgentLoopEventKind::Compact,
        content,
        metadata: json!({
            "event_kind": "compact",
            "reason": reason,
            "requested_by_model": requested_by_model,
            "compacted": compacted,
            "is_error": is_error,
            "tool_call_id": tool_call_id,
            "source_message_count": outcome.map(|value| value.source_message_count),
            "after_message_count": outcome.map(|value| value.compacted_messages.len()),
            "summary_preview": outcome.map(|value| value.summary.compacted_assistant.clone()),
            "budget_report": budget_report,
            "error": error,
        }),
    }
}

fn default_compact_event_metadata() -> Value {
    json!({
        "event_kind": "compact",
    })
}

fn incoming_message(incoming: &IncomingMessage) -> ChatMessage {
    ChatMessage::new(
        ChatMessageRole::User,
        incoming.content.clone(),
        incoming.received_at,
    )
}

fn build_thread_tool_event(
    tool_call: &ToolCallRequest,
    tool_call_id: &str,
    tool_result: &super::ToolCallResult,
) -> ThreadToolEvent {
    let event_kind = match tool_result.metadata["event_kind"].as_str() {
        Some("load_toolset") => ThreadToolEventKind::LoadToolset,
        Some("unload_toolset") => ThreadToolEventKind::UnloadToolset,
        _ => ThreadToolEventKind::ExecuteTool,
    };
    let mut event = ThreadToolEvent::new(event_kind, Utc::now());
    event.toolset_name = tool_result.metadata["toolset"]
        .as_str()
        .map(|name| name.to_string());
    event.tool_name = Some(tool_call.name.clone());
    event.tool_call_id = Some(tool_call_id.to_string());
    event.arguments = Some(tool_call.arguments.clone());
    event.metadata = tool_result.metadata.clone();
    event.is_error = tool_result.is_error;
    event
}

fn build_compact_thread_tool_event(
    tool_call: &ToolCallRequest,
    tool_call_id: &str,
    metadata: Value,
    is_error: bool,
) -> ThreadToolEvent {
    let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, Utc::now());
    event.tool_name = Some(tool_call.name.clone());
    event.tool_call_id = Some(tool_call_id.to_string());
    event.arguments = Some(tool_call.arguments.clone());
    event.metadata = metadata;
    event.is_error = is_error;
    event
}

fn build_compact_provider(
    llm: &Arc<dyn LLMProvider>,
    compact_config: &AgentCompactConfig,
) -> Arc<dyn CompactProvider> {
    if let Some(compacted_assistant) = compact_config.mock_compacted_assistant() {
        info!(
            summary_length = compacted_assistant.len(),
            "using static compact mock provider from config"
        );
        return Arc::new(StaticCompactProvider::new(CompactSummary {
            compacted_assistant: compacted_assistant.to_string(),
        }));
    }

    Arc::new(LLMCompactProvider::new(Arc::clone(llm)))
}
