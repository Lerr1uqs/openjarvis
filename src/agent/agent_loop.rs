//! ReAct-style agent loop that calls the LLM, executes tools, and streams events to the router.

use super::{
    hook::{HookEvent, HookEventKind},
    runtime::AgentRuntime,
    tool::{ToolCallRequest, ToolDefinition},
};
use crate::{
    compact::{
        CompactAllChatStrategy, CompactManager, CompactScopeKey, CompactionOutcome,
        ContextBudgetEstimator, ContextBudgetReport, LLMCompactProvider,
    },
    config::{AgentCompactConfig, LLMConfig},
    context::{ChatMessage, ChatMessageRole, ContextMessage, ContextTokenKind, Messages},
    llm::{LLMProvider, LLMRequest},
    model::ReplyTarget,
    thread::{
        ConversationThread, ThreadCompactToolProjection, ThreadContext, ThreadToolEvent,
        ThreadToolEventKind,
    },
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

pub struct InfoContext {
    pub channel: String,
    pub user_id: String,
    pub thread_id: String,
    pub compact_scope_key: CompactScopeKey,
    pub event_tx: AgentEventSender,
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
    pub turn_messages: Vec<ChatMessage>,
    pub prepend_incoming_user: bool,
    pub thread_context: ThreadContext,
    pub active_thread: ConversationThread,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
}

pub struct AgentLoop {
    llm: Arc<dyn LLMProvider>,
    runtime: AgentRuntime,
    compact_config: AgentCompactConfig,
    budget_estimator: ContextBudgetEstimator,
    compact_manager: CompactManager,
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
        let budget_estimator = ContextBudgetEstimator::from_config(&llm_config, &compact_config);
        let compact_manager = CompactManager::new(
            Arc::new(LLMCompactProvider::new(Arc::clone(&llm))),
            Arc::new(CompactAllChatStrategy),
        );

        Self {
            llm,
            runtime,
            compact_config,
            budget_estimator,
            compact_manager,
        }
    }

    /// Return the runtime used by this loop.
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }

    /// ReAct contract:
    /// 1. `run` 自己维护本轮可变 `messages` 历史，初始值来自 `context.as_messages()`。
    /// 2. 每次循环只调用一次 `llm.generate(messages)`，禁止再走“first/final”两段式专用请求。
    /// 3. 当前轮只要模型返回了可见文本，就立刻通过已绑定用户上下文的 `router_tx` 发送 `text_output` 事件。
    /// 4. 当前轮返回的全部 `tool_calls` 都要逐个发送 `tool_call`、执行工具、发送 `tool_result`，并把 assistant/tool 消息追加回 `messages`。
    /// 5. 只有当本次 `generate` 返回空 `tool_calls` 时才能结束循环；最终回复就是最后一次文本输出。
    pub async fn run(
        &self,
        input: InfoContext,
        context: &ContextMessage,
    ) -> Result<AgentLoopOutput> {
        let thread_context = backfill_thread_context_from_context(&input, context);
        self.run_with_thread_context(input, context, thread_context)
            .await
    }

    /// Run one agent turn with an explicit persisted thread context snapshot.
    pub async fn run_with_thread_context(
        &self,
        input: InfoContext,
        context: &ContextMessage,
        mut thread_context: ThreadContext,
    ) -> Result<AgentLoopOutput> {
        self.runtime.tools().register_builtin_tools().await?;
        self.runtime
            .tools()
            .merge_legacy_thread_state(&mut thread_context)
            .await;
        self.runtime
            .compact_runtime()
            .merge_legacy_scope_overrides(&input.compact_scope_key, &mut thread_context)
            .await;
        let hooks = self.runtime.hooks();
        let thread_id = thread_context.locator.thread_id.clone();
        hooks
            .emit(HookEvent {
                kind: HookEventKind::UserPromptSubmit,
                payload: json!({
                    "channel": input.channel,
                    "user_id": input.user_id,
                    "thread_id": thread_id,
                }),
            })
            .await?;

        let current_user_message = current_user_message_from_context(context)
            .context("agent loop requires one user message")?;
        let mut working_chat_messages = thread_context.load_messages();
        working_chat_messages.push(current_user_message);
        let mut events = Vec::new();
        let mut turn_messages = Vec::new();
        let mut prepend_incoming_user = true;
        let mut used_tool_names = Vec::new();
        let mut last_visible_tools = Vec::new();
        let mut last_budget_report = None;
        let mut skip_next_runtime_compact = false;

        let loop_result = async {
            let reply = loop {
                let toolset_catalog_prompt = self
                    .runtime
                    .tools()
                    .catalog_prompt_for_context(&thread_context)
                    .await;
                let skill_catalog_prompt = self.runtime.tools().skills().catalog_prompt().await;
                let request_state = self
                    .prepare_request_state(
                        &mut thread_context,
                        context,
                        &working_chat_messages,
                        toolset_catalog_prompt.as_deref(),
                        skill_catalog_prompt.as_deref(),
                    )
                    .await?;
                last_visible_tools = request_state.tools.clone();
                last_budget_report = Some(request_state.budget_report.clone());

                if !skip_next_runtime_compact
                    && self.should_runtime_compact(
                        request_state.compact_enabled,
                        &request_state.budget_report,
                    )
                {
                    if let Some(outcome) = self
                        .execute_working_chat_compaction(
                            &hooks,
                            &thread_id,
                            &mut thread_context,
                            &mut working_chat_messages,
                            &mut turn_messages,
                            &mut prepend_incoming_user,
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
                        input.event_tx.send(compact_event.clone()).await?;
                        events.push(compact_event);
                        skip_next_runtime_compact = true;
                        continue;
                    }
                }
                skip_next_runtime_compact = false;

                let RequestState {
                    messages,
                    tools,
                    budget_report,
                    compact_enabled,
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
                    input.event_tx.send(text_event.clone()).await?;
                    events.push(text_event);
                    working_chat_messages.push(assistant_message.clone());
                    turn_messages.push(assistant_message.clone());
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
                    input.event_tx.send(text_event.clone()).await?;
                    events.push(text_event);
                }

                working_chat_messages.push(assistant_tool_message.clone());
                turn_messages.push(assistant_tool_message);

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
                    input.event_tx.send(tool_call_event.clone()).await?;
                    events.push(tool_call_event.clone());
                    hooks
                        .emit(HookEvent {
                            kind: HookEventKind::PreToolUse,
                            payload: tool_call_event.metadata.clone(),
                        })
                        .await?;

                    used_tool_names.push(tool_call.name.clone());
                    if tool_call.name == "compact" {
                        if !compact_enabled {
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
                            input.event_tx.send(compact_event.clone()).await?;
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
                                &mut working_chat_messages,
                                &mut turn_messages,
                                &mut prepend_incoming_user,
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
                                input.event_tx.send(compact_event.clone()).await?;
                                events.push(compact_event.clone());
                                thread_context.record_tool_event(build_compact_thread_tool_event(
                                    &tool_call,
                                    &provider_tool_call.id,
                                    compact_event.metadata.clone(),
                                    false,
                                ));
                                if outcome.is_some() {
                                    skip_next_runtime_compact = true;
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
                                input.event_tx.send(compact_event.clone()).await?;
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
                    input.event_tx.send(tool_result_event.clone()).await?;
                    events.push(tool_result_event);

                    let tool_result_message = ChatMessage::new(
                        ChatMessageRole::ToolResult,
                        tool_result_content,
                        Utc::now(),
                    )
                    .with_tool_call_id(provider_tool_call.id.clone());
                    working_chat_messages.push(tool_result_message.clone());
                    turn_messages.push(tool_result_message);
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
                turn_messages,
                prepend_incoming_user,
                active_thread: thread_context.to_conversation_thread(),
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.pending_tool_events().to_vec(),
                thread_context,
            })
        }
        .await;
        loop_result
    }

    /// Run one agent turn with an explicit legacy `ConversationThread` snapshot.
    #[deprecated(note = "use run_with_thread_context instead")]
    pub async fn run_with_thread(
        &self,
        input: InfoContext,
        context: &ContextMessage,
        active_thread: ConversationThread,
    ) -> Result<AgentLoopOutput> {
        let thread_context = ThreadContext::from_conversation_thread(
            crate::thread::ThreadContextLocator::new(
                None,
                input.channel.clone(),
                input.user_id.clone(),
                input.compact_scope_key.external_thread_id.clone(),
                input.thread_id.clone(),
            ),
            active_thread,
        );
        self.run_with_thread_context(input, context, thread_context)
            .await
    }

    async fn prepare_request_state(
        &self,
        thread_context: &mut ThreadContext,
        context: &ContextMessage,
        working_chat_messages: &[ChatMessage],
        toolset_catalog_prompt: Option<&str>,
        skill_catalog_prompt: Option<&str>,
    ) -> Result<RequestState> {
        let tools = self.runtime.tools();
        let static_tools = tools.list_for_context_static(thread_context).await?;
        let base_messages = build_react_messages(
            &context.system,
            &context.memory,
            working_chat_messages,
            toolset_catalog_prompt,
            skill_catalog_prompt,
            None,
        );
        let base_budget_report = self
            .budget_estimator
            .estimate(&base_messages, &static_tools);

        let compact_enabled = thread_context.compact_enabled(self.compact_config.enabled());
        let auto_compact_enabled = compact_enabled
            && thread_context.auto_compact_enabled(self.compact_config.auto_compact());

        if auto_compact_enabled {
            // 规则: auto_compact 一旦开启，compact 工具就应始终对模型可见，
            // 并且每次 generate 都要注入当前上下文容量提示，让模型自主决定是否提前 compact。
            // tool_visible_threshold_ratio 在这里不控制“是否注入提示”，只控制提示语气是否升级为提前告警。
            thread_context.set_compact_tool_projection(Some(ThreadCompactToolProjection {
                auto_compact: true,
                visible: true,
                budget_report: base_budget_report.clone(),
            }));
            let visible_tools = tools.list_for_context(thread_context).await?;
            let visible_budget_report = self
                .budget_estimator
                .estimate(&base_messages, &visible_tools);
            let auto_compact_prompt = build_auto_compact_prompt(
                &visible_budget_report,
                self.compact_config.tool_visible_threshold_ratio(),
                self.compact_config.runtime_threshold_ratio(),
            );
            let messages = build_react_messages(
                &context.system,
                &context.memory,
                working_chat_messages,
                toolset_catalog_prompt,
                skill_catalog_prompt,
                Some(&auto_compact_prompt),
            );
            let budget_report = self.budget_estimator.estimate(&messages, &visible_tools);
            let refreshed_auto_compact_prompt = build_auto_compact_prompt(
                &budget_report,
                self.compact_config.tool_visible_threshold_ratio(),
                self.compact_config.runtime_threshold_ratio(),
            );
            let messages = build_react_messages(
                &context.system,
                &context.memory,
                working_chat_messages,
                toolset_catalog_prompt,
                skill_catalog_prompt,
                Some(&refreshed_auto_compact_prompt),
            );
            let budget_report = self.budget_estimator.estimate(&messages, &visible_tools);
            thread_context.set_compact_tool_projection(Some(ThreadCompactToolProjection {
                auto_compact: true,
                visible: true,
                budget_report: budget_report.clone(),
            }));

            info!(
                thread_id = %thread_context.locator.thread_id,
                total_estimated_tokens = budget_report.total_estimated_tokens,
                utilization_ratio = budget_report.utilization_ratio,
                tool_count = visible_tools.len(),
                early_warning_reached =
                    budget_report.reaches_ratio(self.compact_config.tool_visible_threshold_ratio()),
                "prepared auto-compact request budget"
            );

            return Ok(RequestState {
                messages,
                tools: visible_tools,
                budget_report,
                compact_enabled,
            });
        }

        thread_context.set_compact_tool_projection(None);
        info!(
            thread_id = %thread_context.locator.thread_id,
            total_estimated_tokens = base_budget_report.total_estimated_tokens,
            utilization_ratio = base_budget_report.utilization_ratio,
            tool_count = static_tools.len(),
            "prepared request budget"
        );

        Ok(RequestState {
            messages: base_messages,
            tools: static_tools,
            budget_report: base_budget_report,
            compact_enabled,
        })
    }

    fn should_runtime_compact(
        &self,
        compact_enabled: bool,
        budget_report: &ContextBudgetReport,
    ) -> bool {
        compact_enabled
            && budget_report.reaches_ratio(self.compact_config.runtime_threshold_ratio())
    }

    async fn call_thread_tool(
        &self,
        thread_context: &mut ThreadContext,
        tool_call: &ToolCallRequest,
    ) -> Result<super::ToolCallResult> {
        self.runtime
            .tools()
            .call_for_context(thread_context, tool_call.clone())
            .await
    }

    async fn execute_working_chat_compaction(
        &self,
        hooks: &Arc<super::HookRegistry>,
        thread_id: &str,
        thread_context: &mut ThreadContext,
        working_chat_messages: &mut Vec<ChatMessage>,
        turn_messages: &mut Vec<ChatMessage>,
        prepend_incoming_user: &mut bool,
        reason: &str,
        budget_report: &ContextBudgetReport,
    ) -> Result<Option<CompactionOutcome>> {
        if working_chat_messages.is_empty() {
            return Ok(None);
        }
        let working_thread =
            build_working_thread(thread_context, working_chat_messages, Utc::now());

        hooks
            .emit(HookEvent {
                kind: HookEventKind::PreCompact,
                payload: json!({
                    "thread_id": thread_id,
                    "reason": reason,
                    "budget_report": budget_report,
                    "active_turn_count": working_thread.turns.len(),
                    "active_message_count": working_chat_messages.len(),
                }),
            })
            .await?;

        info!(
            thread_id,
            reason,
            active_turn_count = working_thread.turns.len(),
            active_message_count = working_chat_messages.len(),
            total_estimated_tokens = budget_report.total_estimated_tokens,
            utilization_ratio = budget_report.utilization_ratio,
            "triggering thread compact"
        );

        let Some(outcome) = self
            .compact_manager
            .compact_thread(&working_thread, Utc::now())
            .await?
        else {
            return Ok(None);
        };
        thread_context.overwrite_active_history_from_conversation_thread(&outcome.compacted_thread);
        *working_chat_messages = thread_context.load_messages();
        turn_messages.clear();
        *prepend_incoming_user = false;

        info!(
            thread_id,
            reason,
            compacted_turn_count = thread_context.conversation.turns.len(),
            compacted_message_count = working_chat_messages.len(),
            "thread compact completed"
        );

        Ok(Some(outcome))
    }
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

fn build_react_messages(
    system_messages: &[ChatMessage],
    memory_messages: &[ChatMessage],
    working_chat_messages: &[ChatMessage],
    toolset_catalog_prompt: Option<&str>,
    skill_catalog_prompt: Option<&str>,
    auto_compact_prompt: Option<&str>,
) -> Messages {
    // Inject runtime instructions into the exact request that will be sent to the LLM.
    let mut messages = Vec::with_capacity(
        system_messages.len() + memory_messages.len() + working_chat_messages.len() + 4,
    );
    messages.extend(system_messages.iter().cloned());
    messages.extend(memory_messages.iter().cloned());
    messages.push(
        ChatMessage::new(
            ChatMessageRole::System,
            "You are running in OpenJarvis tool-use mode. Use the provided tools when needed. You may also provide a short user-visible reply before calling a tool.",
            Utc::now(),
        ),
    );
    if let Some(toolset_catalog_prompt) = toolset_catalog_prompt {
        messages.push(ChatMessage::new(
            ChatMessageRole::System,
            toolset_catalog_prompt,
            Utc::now(),
        ));
    }
    if let Some(skill_catalog_prompt) = skill_catalog_prompt {
        messages.push(ChatMessage::new(
            ChatMessageRole::System,
            skill_catalog_prompt,
            Utc::now(),
        ));
    }
    if let Some(auto_compact_prompt) = auto_compact_prompt {
        messages.push(ChatMessage::new(
            ChatMessageRole::System,
            auto_compact_prompt,
            Utc::now(),
        ));
    }
    messages.extend(working_chat_messages.iter().cloned());
    messages
}

/// Build the runtime auto-compact status prompt injected into every generate while auto-compact is enabled.
///
/// The prompt always exposes the current context usage and the availability of the `compact` tool.
/// `tool_visible_threshold_ratio` only upgrades the wording into an early-warning hint; it does not
/// control whether the prompt exists.
fn build_auto_compact_prompt(
    budget_report: &ContextBudgetReport,
    tool_visible_threshold_ratio: f64,
    runtime_threshold_ratio: f64,
) -> String {
    let token_breakdown = ContextTokenKind::ALL
        .into_iter()
        .map(|kind| format!("{}={}", kind.as_str(), budget_report.tokens(kind)))
        .collect::<Vec<_>>()
        .join(", ");
    let utilization_percent = budget_report.utilization_ratio * 100.0;
    let soft_threshold_percent = tool_visible_threshold_ratio * 100.0;
    let runtime_threshold_percent = runtime_threshold_ratio * 100.0;
    let guidance = if budget_report.reaches_ratio(runtime_threshold_ratio) {
        format!(
            "当前上下文占用已经接近 runtime compact 阈值 ({runtime_threshold_percent:.1}%)，如果你还需要继续消耗上下文，应优先调用 `compact`。"
        )
    } else if budget_report.reaches_ratio(tool_visible_threshold_ratio) {
        format!(
            "当前上下文占用已经超过 auto_compact 提前提醒阈值 ({soft_threshold_percent:.1}%)，应主动考虑尽快调用 `compact`。"
        )
    } else {
        "如果你判断剩余上下文不足以安全继续，可以主动调用 `compact`。".to_string()
    };

    format!(
        "<context capacity {utilization_percent:.1}% used>\nAuto-compact 已开启，`compact` 工具当前可用。\ncurrent_context_budget: {token_breakdown}, total_estimated_tokens={total_estimated_tokens}, context_window_tokens={context_window_tokens}, utilization_ratio={utilization_ratio:.3}, soft_threshold={tool_visible_threshold_ratio:.3}, runtime_threshold={runtime_threshold_ratio:.3}.\n{guidance}\n`compact` 只会压缩当前线程的 chat 历史，不会压缩 system 或 memory。",
        utilization_percent = utilization_percent,
        token_breakdown = token_breakdown,
        total_estimated_tokens = budget_report.total_estimated_tokens,
        context_window_tokens = budget_report.context_window_tokens,
        utilization_ratio = budget_report.utilization_ratio,
        tool_visible_threshold_ratio = tool_visible_threshold_ratio,
        runtime_threshold_ratio = runtime_threshold_ratio,
        guidance = guidance,
    )
}

fn build_working_thread(
    thread_context: &ThreadContext,
    working_chat_messages: &[ChatMessage],
    now: chrono::DateTime<Utc>,
) -> ConversationThread {
    let mut working_thread = thread_context.to_conversation_thread();
    let persisted_message_count = thread_context.load_messages().len();
    let pending_messages = if working_chat_messages.len() > persisted_message_count {
        working_chat_messages[persisted_message_count..].to_vec()
    } else {
        Vec::new()
    };

    if !pending_messages.is_empty() {
        working_thread.store_turn(None, pending_messages, now, now);
    }

    working_thread
}

fn build_compact_event(
    reason: &str,
    requested_by_model: bool,
    is_error: bool,
    budget_report: &ContextBudgetReport,
    outcome: Option<&CompactionOutcome>,
    tool_call_id: Option<&str>,
    error: Option<&str>,
) -> AgentLoopEvent {
    let compacted = outcome.is_some() && !is_error;
    let content = if let Some(error) = error {
        format!("[openjarvis][compact] failed: {error}")
    } else if let Some(outcome) = outcome {
        format!(
            "[openjarvis][compact] compacted current chat history via {}",
            outcome.strategy_name
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
            "strategy_name": outcome.map(|value| value.strategy_name.clone()),
            "source_turn_count": outcome.map(|value| value.plan.source_turn_ids.len()),
            "after_turn_count": outcome.map(|value| value.compacted_thread.turns.len()),
            "summary_preview": outcome.map(|value| value.summary.compacted_assistant.clone()),
            "budget_report": budget_report,
            "error": error,
        }),
    }
}

fn current_user_message_from_context(context: &ContextMessage) -> Option<ChatMessage> {
    context.chat.last().cloned()
}

fn backfill_thread_context_from_context(
    input: &InfoContext,
    context: &ContextMessage,
) -> ThreadContext {
    let now = Utc::now();
    let mut thread_context = ThreadContext::new(
        crate::thread::ThreadContextLocator::new(
            None,
            input.channel.clone(),
            input.user_id.clone(),
            input.compact_scope_key.external_thread_id.clone(),
            input.thread_id.clone(),
        ),
        now,
    );
    if context.chat.len() > 1 {
        thread_context.store_turn(
            None,
            context.chat[..context.chat.len() - 1].to_vec(),
            now,
            now,
        );
    }

    thread_context
}

struct RequestState {
    messages: Messages,
    tools: Vec<ToolDefinition>,
    budget_report: ContextBudgetReport,
    compact_enabled: bool,
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
