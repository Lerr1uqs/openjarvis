//! ReAct-style agent loop that calls the LLM, executes tools, and streams events to the router.

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
    thread::{Thread, ThreadToolEvent, ThreadToolEventKind},
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc;
use tracing::info;

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

#[derive(Clone)]
pub struct AgentEventSender {
    router_tx: mpsc::Sender<AgentDispatchEvent>,
    channel: String,
    external_thread_id: Option<String>,
    source_message_id: Option<String>,
    target: ReplyTarget,
    session_id: String,
    session_channel: String,
    session_user_id: String,
    session_external_thread_id: String,
    session_thread_id: String,
    should_reply_to_source: Arc<AtomicBool>,
}

impl AgentEventSender {
    /// Bind the router sender to one user/session context so agent events can be emitted directly.
    pub fn new(
        router_tx: mpsc::Sender<AgentDispatchEvent>,
        channel: impl Into<String>,
        external_thread_id: Option<String>,
        source_message_id: Option<String>,
        target: ReplyTarget,
        session_id: impl Into<String>,
        session_channel: impl Into<String>,
        session_user_id: impl Into<String>,
        session_external_thread_id: impl Into<String>,
        session_thread_id: impl Into<String>,
    ) -> Self {
        Self {
            router_tx,
            channel: channel.into(),
            external_thread_id,
            source_message_id,
            target,
            session_id: session_id.into(),
            session_channel: session_channel.into(),
            session_user_id: session_user_id.into(),
            session_external_thread_id: session_external_thread_id.into(),
            session_thread_id: session_thread_id.into(),
            should_reply_to_source: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Bind the router sender from one resolved thread locator and the current incoming message.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentEventSender,
    ///     model::{IncomingMessage, ReplyTarget},
    ///     thread::ThreadContextLocator,
    /// };
    /// use serde_json::json;
    /// use tokio::sync::mpsc;
    /// use uuid::Uuid;
    ///
    /// let (tx, _rx) = mpsc::channel(1);
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
    /// let _sender = AgentEventSender::from_incoming_and_locator(tx, &incoming, &locator);
    /// ```
    pub fn from_incoming_and_locator(
        router_tx: mpsc::Sender<AgentDispatchEvent>,
        incoming: &IncomingMessage,
        locator: &crate::thread::ThreadContextLocator,
    ) -> Self {
        Self::new(
            router_tx,
            incoming.channel.clone(),
            incoming.external_thread_id.clone(),
            incoming.external_message_id.clone(),
            incoming.reply_target.clone(),
            locator.session_id.clone().unwrap_or_default(),
            locator.channel.clone(),
            locator.user_id.clone(),
            locator.external_thread_id.clone(),
            locator.thread_id.clone(),
        )
    }

    /// Convert one agent-loop event into a structured dispatch event for the router.
    pub async fn send(&self, event: AgentLoopEvent) -> Result<()> {
        let reply_to_source = self.should_reply_to_source.swap(false, Ordering::AcqRel);

        self.router_tx
            .send(AgentDispatchEvent {
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
            })
            .await
            .map_err(|error| anyhow::anyhow!("failed to forward agent event to router: {}", error))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLoopEventKind {
    TextOutput,
    ToolCall,
    ToolResult,
    Compact,
}

#[derive(Debug, Clone)]
pub struct AgentLoopEvent {
    pub kind: AgentLoopEventKind,
    pub content: String,
    pub metadata: Value,
}

pub struct AgentLoopOutput {
    pub reply: String,
    pub metadata: Value,
    pub events: Vec<AgentLoopEvent>,
    pub commit_messages: Vec<ChatMessage>,
    pub persist_incoming_user: bool,
    pub thread_context: Thread,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
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
        Self::with_compact_config_and_system_prompt(
            llm,
            runtime,
            LLMConfig::default(),
            AgentCompactConfig::default(),
            None::<String>,
        )
    }

    /// Create an agent loop with explicit compact and budget config.
    pub fn with_compact_config(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
    ) -> Self {
        Self::with_compact_config_and_system_prompt(
            llm,
            runtime,
            llm_config,
            compact_config,
            None::<String>,
        )
    }

    /// Create an agent loop with explicit compact config and one thread-init system prompt.
    pub fn with_compact_config_and_system_prompt(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
        thread_system_prompt: impl Into<Option<String>>,
    ) -> Self {
        let compact_provider = build_compact_provider(&llm, &compact_config);
        Self::with_compact_provider_and_system_prompt(
            llm,
            runtime,
            llm_config,
            compact_config,
            compact_provider,
            thread_system_prompt,
        )
    }

    /// Create an agent loop with an explicitly injected compact provider.
    pub fn with_compact_provider(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
        compact_provider: Arc<dyn CompactProvider>,
    ) -> Self {
        Self::with_compact_provider_and_system_prompt(
            llm,
            runtime,
            llm_config,
            compact_config,
            compact_provider,
            None::<String>,
        )
    }

    /// Create an agent loop with an explicitly injected compact provider and thread-init prompt.
    pub fn with_compact_provider_and_system_prompt(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
        compact_provider: Arc<dyn CompactProvider>,
        _thread_system_prompt: impl Into<Option<String>>,
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

    /// ReAct contract:
    /// 1. `run_v1` 接收 `Thread + 当前 incoming`，由 loop 自己维护本轮可变 `messages` 历史。
    /// 2. 每次循环只调用一次 `llm.generate(messages)`，禁止再走“first/final”两段式专用请求。
    /// 3. 当前轮只要模型返回了可见文本，就立刻通过已绑定用户上下文的 `router_tx` 发送 `text_output` 事件。
    /// 4. 当前轮返回的全部 `tool_calls` 都要逐个发送 `tool_call`、执行工具、发送 `tool_result`，并把 assistant/tool 消息追加回 `messages`。
    /// 5. 只有当本次 `generate` 返回空 `tool_calls` 时才能结束循环；最终回复就是最后一次文本输出。
    pub async fn run_v1(
        &self,
        event_tx: AgentEventSender,
        incoming: &IncomingMessage,
        thread_context: Thread,
    ) -> Result<AgentLoopOutput> {
        self.run_live_thread(event_tx, thread_context, incoming_message(incoming))
            .await
    }

    async fn run_live_thread(
        &self,
        event_tx: AgentEventSender,
        thread_context: Thread,
        current_message: ChatMessage,
    ) -> Result<AgentLoopOutput> {
        let mut thread_context = thread_context;
        let mut request_system_messages = Vec::new();
        let mut live_chat_messages = Vec::new();
        self.prepare_thread_runtime(&mut thread_context).await?;
        live_chat_messages.push(current_message);
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

        let mut events = Vec::new();
        let mut commit_messages = Vec::new();
        let mut persist_incoming_user = true;
        let mut used_tool_names = Vec::new();
        let mut last_visible_tools = Vec::new();
        let mut last_budget_report = None;

        let loop_result = async {
            let reply = loop {
                let request_state = self
                    .prepare_request_state(
                        &thread_context,
                        &mut request_system_messages,
                        &live_chat_messages,
                    )
                    .await?;
                last_visible_tools = request_state.tools.clone();
                last_budget_report = Some(request_state.budget_report.clone());

                if self.should_runtime_compact(&live_chat_messages, &request_state.budget_report) {
                    if let Some(outcome) = self
                        .execute_working_chat_compaction(
                            &hooks,
                            &thread_id,
                            &mut thread_context,
                            &mut request_system_messages,
                            &mut live_chat_messages,
                            &mut commit_messages,
                            &mut persist_incoming_user,
                            "runtime_threshold",
                            &request_state.budget_report,
                        )
                        .await?
                    {
                        let compact_event = build_compact_event(
                            "runtime_threshold",
                            false,
                            false,
                            &request_state.budget_report,
                            Some(&outcome),
                            None,
                            None,
                        );
                        event_tx.send(compact_event.clone()).await?;
                        events.push(compact_event);
                        continue;
                    }
                }

                let RequestState {
                    messages,
                    tools,
                    budget_report,
                } = request_state;

                let response = self.llm.generate(LLMRequest { messages, tools }).await?;

                if response.tool_calls.is_empty() {
                    let assistant_message = response
                        .message
                        .context("llm response did not contain assistant text or tool calls")?;
                    if assistant_message.content.trim().is_empty() {
                        bail!("llm final response did not contain assistant text");
                    }

                    let text_event = AgentLoopEvent {
                        kind: AgentLoopEventKind::TextOutput,
                        content: assistant_message.content.clone(),
                        metadata: json!({
                            "source": "llm_response",
                            "is_final": true,
                        }),
                    };
                    event_tx.send(text_event.clone()).await?;
                    events.push(text_event);
                    live_chat_messages.push(assistant_message.clone());
                    commit_messages.push(assistant_message.clone());
                    break assistant_message.content;
                }

                let assistant_tool_message = build_assistant_tool_call_message(
                    response.message.as_ref(),
                    &response.tool_calls,
                );
                if let Some(message) = response.message.as_ref()
                    && !message.content.trim().is_empty()
                {
                    let text_event = AgentLoopEvent {
                        kind: AgentLoopEventKind::TextOutput,
                        content: message.content.clone(),
                        metadata: json!({
                            "source": "llm_response",
                            "is_final": false,
                        }),
                    };
                    event_tx.send(text_event.clone()).await?;
                    events.push(text_event);
                }

                live_chat_messages.push(assistant_tool_message.clone());
                commit_messages.push(assistant_tool_message);

                let mut restart_loop_after_compaction = false;
                for provider_tool_call in response.tool_calls {
                    let tool_call = ToolCallRequest {
                        name: provider_tool_call.name.clone(),
                        arguments: provider_tool_call.arguments.clone(),
                    };
                    let tool_call_event = AgentLoopEvent {
                        kind: AgentLoopEventKind::ToolCall,
                        content: format!(
                            "[openjarvis][tool_call] {} {}",
                            tool_call.name, tool_call.arguments
                        ),
                        metadata: json!({
                            "tool": tool_call.name,
                            "arguments": tool_call.arguments,
                            "tool_call_id": provider_tool_call.id.clone(),
                        }),
                    };
                    event_tx.send(tool_call_event.clone()).await?;
                    events.push(tool_call_event.clone());
                    hooks
                        .emit(HookEvent {
                            kind: HookEventKind::PreToolUse,
                            payload: tool_call_event.metadata.clone(),
                        })
                        .await?;

                    used_tool_names.push(tool_call.name.clone());
                    if tool_call.name == "compact" {
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
                            let compact_event = build_compact_event(
                                "tool_requested",
                                true,
                                true,
                                &budget_report,
                                None,
                                Some(&provider_tool_call.id),
                                Some(&error_message),
                            );
                            event_tx.send(compact_event.clone()).await?;
                            events.push(compact_event.clone());
                            thread_context.record_tool_event(build_compact_thread_tool_event(
                                &tool_call,
                                &provider_tool_call.id,
                                compact_event.metadata.clone(),
                                true,
                            ));
                            continue;
                        }

                        match self
                            .execute_working_chat_compaction(
                                &hooks,
                                &thread_id,
                                &mut thread_context,
                                &mut request_system_messages,
                                &mut live_chat_messages,
                                &mut commit_messages,
                                &mut persist_incoming_user,
                                "tool_requested",
                                &budget_report,
                            )
                            .await
                        {
                            Ok(outcome) => {
                                let compact_metadata = build_compact_event(
                                    "tool_requested",
                                    true,
                                    false,
                                    &budget_report,
                                    outcome.as_ref(),
                                    Some(&provider_tool_call.id),
                                    None,
                                )
                                .metadata
                                .clone();
                                hooks
                                    .emit(HookEvent {
                                        kind: HookEventKind::PostToolUse,
                                        payload: json!({
                                            "tool": tool_call.name.clone(),
                                            "result": compact_metadata,
                                        }),
                                    })
                                    .await?;
                                let compact_event = build_compact_event(
                                    "tool_requested",
                                    true,
                                    false,
                                    &budget_report,
                                    outcome.as_ref(),
                                    Some(&provider_tool_call.id),
                                    None,
                                );
                                event_tx.send(compact_event.clone()).await?;
                                events.push(compact_event.clone());
                                thread_context.record_tool_event(build_compact_thread_tool_event(
                                    &tool_call,
                                    &provider_tool_call.id,
                                    compact_event.metadata.clone(),
                                    false,
                                ));
                                if outcome.is_some() {
                                    restart_loop_after_compaction = true;
                                    break;
                                }
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
                                let compact_event = build_compact_event(
                                    "tool_requested",
                                    true,
                                    true,
                                    &budget_report,
                                    None,
                                    Some(&provider_tool_call.id),
                                    Some(&error_message),
                                );
                                event_tx.send(compact_event.clone()).await?;
                                events.push(compact_event.clone());
                                thread_context.record_tool_event(build_compact_thread_tool_event(
                                    &tool_call,
                                    &provider_tool_call.id,
                                    compact_event.metadata.clone(),
                                    true,
                                ));
                            }
                        }
                        continue;
                    }

                    let tool_result =
                        match self.call_thread_tool(&mut thread_context, &tool_call).await {
                            Ok(result) => {
                                let result_metadata = result.metadata.clone();
                                hooks
                                    .emit(HookEvent {
                                        kind: HookEventKind::PostToolUse,
                                        payload: json!({
                                            "tool": tool_call.name.clone(),
                                            "result": result_metadata,
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

                    let tool_result_content =
                        format_tool_result_content(&tool_result.content, tool_result.is_error);
                    let tool_result_event = AgentLoopEvent {
                        kind: AgentLoopEventKind::ToolResult,
                        content: format!("[openjarvis][tool_result] {}", tool_result_content),
                        metadata: json!({
                            "tool": tool_call.name.clone(),
                            "is_error": tool_result.is_error,
                            "metadata": tool_result.metadata,
                            "tool_call_id": provider_tool_call.id.clone(),
                        }),
                    };
                    event_tx.send(tool_result_event.clone()).await?;
                    events.push(tool_result_event);

                    let tool_result_message = ChatMessage::new(
                        ChatMessageRole::ToolResult,
                        tool_result_content,
                        Utc::now(),
                    )
                    .with_tool_call_id(provider_tool_call.id.clone());
                    live_chat_messages.push(tool_result_message.clone());
                    commit_messages.push(tool_result_message);
                }

                if restart_loop_after_compaction {
                    continue;
                }
            };

            let mcp_server_count = self.runtime.tools().mcp().list_servers().await.len();
            let hook_handler_count = hooks.len().await;
            let metadata = json!({
                "tool_count": last_visible_tools.len(),
                "mcp_server_count": mcp_server_count,
                "hook_handler_count": hook_handler_count,
                "used_tool_name": used_tool_names.first().cloned(),
                "used_tool_names": used_tool_names,
                "loaded_toolsets": thread_context.load_toolsets(),
                "event_count": events.len(),
                "context_budget": last_budget_report,
            });

            hooks
                .emit(HookEvent {
                    kind: HookEventKind::Notification,
                    payload: json!({
                        "reply_preview": reply,
                        "runtime": metadata,
                    }),
                })
                .await?;

            Ok(AgentLoopOutput {
                reply,
                metadata,
                events,
                commit_messages,
                persist_incoming_user,
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.pending_tool_events().to_vec(),
                thread_context,
            })
        }
        .await;
        loop_result
    }

    fn auto_compact_enabled_for_thread(&self, thread_context: &Thread) -> bool {
        self.compact_config.enabled()
            && thread_context.auto_compact_enabled(self.compact_config.auto_compact())
    }

    async fn prepare_thread_runtime(&self, thread_context: &mut Thread) -> Result<()> {
        info!(
            thread_id = %thread_context.locator.thread_id,
            "preparing thread runtime"
        );
        self.runtime.tools().register_builtin_tools().await?;
        Ok(())
    }

    async fn prepare_request_tools_ex(&self, thread_context: &Thread) -> Result<ThreadInitStateEx> {
        let auto_compact_enabled = self.auto_compact_enabled_for_thread(thread_context);

        let tools = if auto_compact_enabled {
            self.runtime.list_tools(thread_context, true).await?
        } else {
            self.runtime.list_tools(thread_context, false).await?
        };

        info!(
            thread_id = %thread_context.locator.thread_id,
            auto_compact_enabled,
            tool_count = tools.len(),
            "initialized experimental agent-loop thread request state"
        );

        // TODO: auto_compact 后续应改成只能在创建新 Thread 时决定，
        // 不再允许在已有 ctx 生命周期中途打开，避免 request-state 分支继续膨胀。
        Ok(ThreadInitStateEx {
            auto_compact_enabled,
            tools,
        })
    }

    async fn prepare_request_state(
        &self,
        thread_context: &Thread,
        request_system_messages: &mut Vec<ChatMessage>,
        live_chat_messages: &[ChatMessage],
    ) -> Result<RequestState> {
        let init_state = self.prepare_request_tools_ex(thread_context).await?;
        let ThreadInitStateEx {
            auto_compact_enabled,
            tools,
        } = init_state;

        let (messages, budget_report) = if auto_compact_enabled {
            self.refresh_auto_compact_request_ex(
                thread_context,
                request_system_messages,
                live_chat_messages,
                &tools,
            )
        } else {
            self.auto_compactor
                .notify_capacity(request_system_messages, None);
            let messages =
                build_request_messages(thread_context, request_system_messages, live_chat_messages);
            let budget_report = self.budget_estimator.estimate(&messages, &tools);
            (messages, budget_report)
        };

        if auto_compact_enabled {
            info!(
                thread_id = %thread_context.locator.thread_id,
                total_estimated_tokens = budget_report.total_estimated_tokens,
                utilization_ratio = budget_report.utilization_ratio,
                tool_count = tools.len(),
                early_warning_reached =
                    budget_report.reaches_ratio(self.compact_config.tool_visible_threshold_ratio()),
                "prepared auto-compact request budget"
            );
        } else {
            info!(
                thread_id = %thread_context.locator.thread_id,
                total_estimated_tokens = budget_report.total_estimated_tokens,
                utilization_ratio = budget_report.utilization_ratio,
                tool_count = tools.len(),
                "prepared request budget"
            );
        }

        Ok(RequestState {
            messages,
            tools,
            budget_report,
        })
    }

    fn should_runtime_compact(
        &self,
        live_chat_messages: &[ChatMessage],
        budget_report: &ContextBudgetReport,
    ) -> bool {
        self.compact_config.enabled()
            && !live_chat_messages.is_empty()
            && budget_report.reaches_ratio(self.compact_config.runtime_threshold_ratio())
    }

    async fn call_thread_tool(
        &self,
        thread_context: &mut Thread,
        tool_call: &ToolCallRequest,
    ) -> Result<super::ToolCallResult> {
        self.runtime
            .call_tool(thread_context, tool_call.clone())
            .await
    }

    async fn execute_working_chat_compaction(
        &self,
        hooks: &Arc<super::HookRegistry>,
        thread_id: &str,
        thread_context: &mut Thread,
        request_system_messages: &mut Vec<ChatMessage>,
        live_chat_messages: &mut Vec<ChatMessage>,
        commit_messages: &mut Vec<ChatMessage>,
        persist_incoming_user: &mut bool,
        reason: &str,
        budget_report: &ContextBudgetReport,
    ) -> Result<Option<MessageCompactionOutcome>> {
        let mut compactable_messages = thread_context.load_messages();
        compactable_messages.extend(live_chat_messages.iter().cloned());
        if compactable_messages.is_empty() {
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
        thread_context.replace_non_system_messages(outcome.compacted_messages.clone(), Utc::now());
        request_system_messages.clear();
        live_chat_messages.clear();
        commit_messages.clear();
        *persist_incoming_user = false;

        info!(
            thread_id,
            reason,
            compacted_message_count = thread_context.load_messages().len(),
            "thread compact completed"
        );

        Ok(Some(outcome))
    }
    fn refresh_auto_compact_request_ex(
        &self,
        thread_context: &Thread,
        request_system_messages: &mut Vec<ChatMessage>,
        live_chat_messages: &[ChatMessage],
        visible_tools: &[ToolDefinition],
    ) -> (Messages, ContextBudgetReport) {
        let mut budget_report = self.budget_estimator.estimate(
            &build_request_messages(thread_context, request_system_messages, live_chat_messages),
            visible_tools,
        );

        // 动态容量提示本身也会消耗上下文，所以这里固定进行两轮收敛，
        // 让最终预算快照和最终请求消息尽量一致。
        for _ in 0..2 {
            self.auto_compactor
                .notify_capacity(request_system_messages, Some(&budget_report));
            budget_report = self.budget_estimator.estimate(
                &build_request_messages(
                    thread_context,
                    request_system_messages,
                    live_chat_messages,
                ),
                visible_tools,
            );
        }

        self.auto_compactor
            .notify_capacity(request_system_messages, Some(&budget_report));

        (
            build_request_messages(thread_context, request_system_messages, live_chat_messages),
            budget_report,
        )
    }
}

fn build_request_messages(
    thread_context: &Thread,
    request_system_messages: &[ChatMessage],
    live_chat_messages: &[ChatMessage],
) -> Messages {
    let mut messages = Vec::with_capacity(
        thread_context.messages().len() + request_system_messages.len() + live_chat_messages.len(),
    );
    messages.extend(thread_context.messages());
    messages.extend(request_system_messages.iter().cloned());
    messages.extend(live_chat_messages.iter().cloned());
    messages
}

fn build_assistant_tool_call_message(
    assistant_message: Option<&ChatMessage>,
    tool_calls: &[crate::llm::LLMToolCall],
) -> ChatMessage {
    // Preserve the original assistant tool-call message so persisted history can be replayed verbatim.
    let created_at = assistant_message
        .map(|message| message.created_at)
        .unwrap_or_else(Utc::now);
    let content = assistant_message
        .map(|message| message.content.clone())
        .unwrap_or_default();

    ChatMessage::new(ChatMessageRole::Assistant, content, created_at)
        .with_tool_calls(tool_calls.to_vec())
}

fn format_tool_result_content(content: &str, is_error: bool) -> String {
    // Keep tool result text identical between immediate replies and persisted history.
    if is_error {
        return format!("Tool execution failed: {content}");
    }

    content.to_string()
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

fn incoming_message(incoming: &IncomingMessage) -> ChatMessage {
    ChatMessage::new(
        ChatMessageRole::User,
        incoming.content.clone(),
        incoming.received_at,
    )
}

struct RequestState {
    messages: Messages,
    tools: Vec<ToolDefinition>,
    budget_report: ContextBudgetReport,
}

struct ThreadInitStateEx {
    auto_compact_enabled: bool,
    tools: Vec<ToolDefinition>,
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
