use super::{
    hook::{HookEvent, HookEventKind},
    runtime::AgentRuntime,
    tool::ToolCallRequest,
};
use crate::{
    context::{ChatMessage, ChatMessageRole, ContextMessage, Messages},
    llm::{LLMProvider, LLMRequest},
    model::{OutgoingMessage, ReplyTarget},
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone)]
pub struct AgentEventSender {
    router_tx: mpsc::Sender<OutgoingMessage>,
    channel: String,
    thread_id: String,
    source_message_id: Option<String>,
    target: ReplyTarget,
    session_channel: String,
    session_user_id: String,
    session_thread_id: String,
    should_reply_to_source: Arc<AtomicBool>,
}

impl AgentEventSender {
    pub fn new(
        router_tx: mpsc::Sender<OutgoingMessage>,
        channel: impl Into<String>,
        thread_id: impl Into<String>,
        source_message_id: Option<String>,
        target: ReplyTarget,
        session_channel: impl Into<String>,
        session_user_id: impl Into<String>,
        session_thread_id: impl Into<String>,
    ) -> Self {
        // 作用: 绑定 router tx 和当前用户上下文，供 agent loop 直接推送事件消息。
        // 参数: router_tx 为发往 router 的出站消息通道，其余字段用于生成统一 OutgoingMessage。
        Self {
            router_tx,
            channel: channel.into(),
            thread_id: thread_id.into(),
            source_message_id,
            target,
            session_channel: session_channel.into(),
            session_user_id: session_user_id.into(),
            session_thread_id: session_thread_id.into(),
            should_reply_to_source: Arc::new(AtomicBool::new(true)),
        }
    }

    pub async fn send(&self, event: AgentLoopEvent) -> Result<()> {
        // 作用: 把 agent loop 事件转换成统一出站消息，并立即转发给 router。
        // 参数: event 为当前轮产生的 text_output、tool_call 或 tool_result 事件。
        let reply_to_message_id = if self.should_reply_to_source.swap(false, Ordering::AcqRel) {
            self.source_message_id.clone()
        } else {
            None
        };

        self.router_tx
            .send(OutgoingMessage {
                id: Uuid::new_v4(),
                channel: self.channel.clone(),
                content: event.content,
                thread_id: Some(self.thread_id.clone()),
                metadata: json!({
                    "source_message_id": self.source_message_id,
                    "session_channel": self.session_channel,
                    "session_user_id": self.session_user_id,
                    "session_thread_id": self.session_thread_id,
                    "event_kind": format!("{:?}", event.kind),
                    "event_metadata": event.metadata,
                }),
                reply_to_message_id,
                target: self.target.clone(),
            })
            .await
            .map_err(|error| anyhow::anyhow!("failed to forward agent event to router: {}", error))
    }
}

pub struct AgentLoopInput {
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
    pub fn new(llm: Arc<dyn LLMProvider>, runtime: AgentRuntime) -> Self {
        // 作用: 创建 agent loop 执行器，并绑定模型与运行时 registries。
        // 参数: llm 为当前 loop 使用的模型提供者，runtime 为 hook/tool/mcp 运行时容器。
        Self { llm, runtime }
    }

    pub fn runtime(&self) -> &AgentRuntime {
        // 作用: 返回当前 agent loop 绑定的运行时，用于向外暴露 registries。
        // 参数: 无，返回当前 loop 内部持有的 runtime 引用。
        &self.runtime
    }

    /**
    ReAct contract:
    1. `run` 自己维护本轮可变 `messages` 历史，初始值来自 `context.as_messages()`。
    2. 每次循环只调用一次 `llm.generate(messages)`，禁止再走“first/final”两段式专用请求。
    3. 当前轮只要模型返回了可见文本，就立刻通过已绑定用户上下文的 `router_tx` 发送 `text_output` 事件。
    4. 当前轮返回的全部 `tool_calls` 都要逐个发送 `tool_call`、执行工具、发送 `tool_result`，并把 assistant/tool 消息追加回 `messages`。
    5. 只有当本次 `generate` 返回空 `tool_calls` 时才能结束循环；最终回复就是最后一次文本输出。
    */
    pub async fn run(
        &self,
        input: AgentLoopInput,
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
        let mut events = Vec::new();
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

            for llm_tool_call in response.tool_calls {
                let tool_call = ToolCallRequest {
                    name: llm_tool_call.name.clone(),
                    arguments: llm_tool_call.arguments.clone(),
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
                        "tool_call_id": llm_tool_call.id,
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
                        "tool_call_id": llm_tool_call.id,
                    }),
                };
                input.event_tx.send(tool_result_event.clone()).await?;
                events.push(tool_result_event);

                let tool_result_message =
                    ChatMessage::new(ChatMessageRole::ToolResult, tool_result_content, Utc::now())
                        .with_tool_call_id(llm_tool_call.id.clone());
                messages.push(tool_result_message.clone());
                turn_messages.push(tool_result_message);
            }
        };

        let tool_count = self.runtime.tools().list().await.len();
        let mcp_server_count = self.runtime.mcp().list().await.len();
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
    // 作用: 为持久化层构造 assistant 的原始 tool-call message，保证历史上下文可原样回放。
    // 参数: assistant_message 为模型返回的可见文本部分，tool_calls 为模型原生函数调用列表。
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
    // 作用: 统一生成发送给模型和持久化的 tool result 文本，避免当前轮与历史回放不一致。
    // 参数: content 为工具执行结果原文，is_error 表示本次工具执行是否失败。
    if is_error {
        return format!("Tool execution failed: {content}");
    }

    content.to_string()
}

fn build_react_messages(context: &ContextMessage) -> Messages {
    // 作用: 把当前上下文和工具使用约束拼接成第一次 LLM 请求消息列表。
    // 参数: context 为当前上下文消息。
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
