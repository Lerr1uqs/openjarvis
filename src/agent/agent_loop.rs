//! ReAct-style agent loop that calls the LLM, executes tools, and streams events to the router.

use super::{
    hook::{HookEvent, HookEventKind},
    runtime::AgentRuntime,
    tool::ToolCallRequest,
};
use crate::{
    context::{ChatMessage, ChatMessageRole, ContextMessage, Messages},
    llm::{LLMProvider, LLMRequest},
    model::ReplyTarget,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct AgentDispatchEvent {
    pub kind: AgentLoopEventKind,
    pub content: String,
    pub metadata: Value,
    pub channel: String,
    pub thread_id: Option<String>,
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
    thread_id: Option<String>,
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
        thread_id: Option<String>,
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
            thread_id,
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
                thread_id: self.thread_id.clone(),
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
    pub event_tx: AgentEventSender,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLoopEventKind {
    TextOutput,
    ToolCall,
    ToolResult,
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
}

pub struct AgentLoop {
    llm: Arc<dyn LLMProvider>,
    runtime: AgentRuntime,
}

impl AgentLoop {
    /// Create an agent loop bound to one LLM provider and runtime container.
    pub fn new(llm: Arc<dyn LLMProvider>, runtime: AgentRuntime) -> Self {
        Self { llm, runtime }
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
        self.runtime.tools().register_builtin_tools().await?;
        let hooks = self.runtime.hooks();
        hooks
            .emit(HookEvent {
                kind: HookEventKind::UserPromptSubmit,
                payload: json!({
                    "channel": input.channel,
                    "user_id": input.user_id,
                    "thread_id": input.thread_id,
                }),
            })
            .await?;

        let tools = self.runtime.tools().list().await;
        let mut messages = build_react_messages(context);
        let mut events = Vec::new(); // events有无必要？
        let mut turn_messages = Vec::new();
        let mut used_tool_names = Vec::new();

        let reply = loop {
            let response = self
                .llm
                .generate(LLMRequest {
                    messages: messages.clone(),
                    tools: tools.clone(),
                })
                .await?;

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
                messages.push(assistant_message.clone());
                turn_messages.push(assistant_message.clone());
                break assistant_message.content;
            }

            let assistant_tool_message =
                build_assistant_tool_call_message(response.message.as_ref(), &response.tool_calls);
            if let Some(message) = response.message.as_ref() {
                if !message.content.trim().is_empty() {
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
            }

            messages.push(assistant_tool_message.clone());
            turn_messages.push(assistant_tool_message);

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
                let tool_result = self.runtime.tools().call(tool_call.clone()).await;
                let tool_result = match tool_result {
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

                let tool_result_message =
                    ChatMessage::new(ChatMessageRole::ToolResult, tool_result_content, Utc::now())
                        .with_tool_call_id(provider_tool_call.id.clone());
                messages.push(tool_result_message.clone());
                turn_messages.push(tool_result_message);
            }
        };

        let tool_count = self.runtime.tools().list().await.len();
        let mcp_server_count = self.runtime.tools().mcp().list_servers().await.len();
        let hook_handler_count = hooks.len().await;
        let metadata = json!({
            "tool_count": tool_count,
            "mcp_server_count": mcp_server_count,
            "hook_handler_count": hook_handler_count,
            "used_tool_name": used_tool_names.first().cloned(),
            "used_tool_names": used_tool_names,
            "event_count": events.len(),
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
        })
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
// TODO: 这里直接用Vec<ChatMessage>就行了
fn build_react_messages(context: &ContextMessage) -> Messages {
    // Inject a tool-usage instruction into the first request assembled from the current context.
    let mut messages = context.as_messages();
    let insert_at = context.system.len() + context.memory.len();
    messages.insert(
        insert_at,
        ChatMessage::new(
            ChatMessageRole::System,
            "You are running in OpenJarvis tool-use mode. Use the provided tools when needed. You may also provide a short user-visible reply before calling a tool.",
            Utc::now(),
        ),
    );
    messages
}
