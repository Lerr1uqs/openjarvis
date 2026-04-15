use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentEventSender, AgentLoop, AgentLoopOutput, AgentRuntime, ToolCallRequest,
        ToolCallResult, ToolDefinition, ToolHandler, empty_tool_input_schema,
    },
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_agent_loop".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_agent_loop".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_agent_loop".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_agent_loop".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread(now: chrono::DateTime<Utc>) -> Thread {
    Thread::new(
        ThreadContextLocator::new(
            Some(Uuid::new_v4().to_string()),
            "feishu",
            "ou_agent_loop",
            "chat_agent_loop",
            Uuid::new_v4().to_string(),
        ),
        now,
    )
}

struct ScriptedLLMProvider {
    responses: Mutex<VecDeque<LLMResponse>>,
}

impl ScriptedLLMProvider {
    fn new(responses: Vec<LLMResponse>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }
}

#[async_trait]
impl LLMProvider for ScriptedLLMProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("no scripted response left"))
    }
}

struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo tool for loop tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "echo-result".to_string(),
            metadata: json!({}),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn agent_loop_returns_success_output_without_turn_payload() {
    // 测试场景: loop 只返回请求结果与 metadata，不再返回 finalized turn payload。
    let incoming = build_incoming("hello");
    let thread = build_thread(incoming.received_at);
    let loop_runtime = AgentRuntime::new();
    let loop_instance = AgentLoop::new(Arc::new(MockLLMProvider::new("loop-reply")), loop_runtime);

    let output = loop_instance
        .run_v1(
            AgentEventSender::from_incoming_and_locator(&incoming, &thread.locator),
            &incoming,
            thread,
        )
        .await
        .expect("agent loop should succeed");

    assert_eq!(output.reply, "loop-reply");
    assert!(output.succeeded);
    assert_eq!(output.metadata["request_status"], "succeeded");
}

#[tokio::test]
async fn agent_loop_emits_tool_messages_without_persisting_tool_audit_state() {
    // 测试场景: 工具调用结果应直接写入正式消息序列，而不是额外堆积线程级 tool audit 状态。
    let incoming = build_incoming("hello");
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(EchoTool))
        .await
        .expect("tool should register");
    let loop_instance = AgentLoop::new(
        Arc::new(ScriptedLLMProvider::new(vec![
            LLMResponse {
                items: vec![
                    ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now()).with_tool_calls(
                        vec![LLMToolCall {
                            id: "call_1".to_string(),
                            name: "demo__echo".to_string(),
                            arguments: json!({}),
                            provider_item_id: None,
                        }],
                    ),
                ],
            },
            LLMResponse {
                items: vec![ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "tool finished",
                    Utc::now(),
                )],
            },
        ])),
        runtime,
    );
    let mut thread = build_thread(incoming.received_at);

    let AgentLoopOutput {
        reply, succeeded, ..
    } = loop_instance
        .run_locked_thread(
            AgentEventSender::from_incoming_and_locator(&incoming, &thread.locator),
            &incoming,
            &mut thread,
            &mut DummyCommittedHandler,
        )
        .await
        .expect("agent loop should succeed");

    assert_eq!(reply, "tool finished");
    assert!(succeeded);
    assert_eq!(
        thread
            .messages()
            .into_iter()
            .map(|message| message.role)
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::User,
            ChatMessageRole::Toolcall,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
}

struct DummyCommittedHandler;

#[async_trait]
impl openjarvis::agent::agent_loop::AgentCommittedMessageHandler for DummyCommittedHandler {
    async fn on_committed_message(
        &mut self,
        _thread_context: &mut Thread,
        _message: ChatMessage,
        _dispatch_events: Vec<openjarvis::agent::AgentDispatchEvent>,
    ) -> Result<()> {
        Ok(())
    }
}
