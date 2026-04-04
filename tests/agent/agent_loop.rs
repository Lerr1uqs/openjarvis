use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEventKind, AgentRuntime,
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, empty_tool_input_schema,
    },
    config::{AgentCompactConfig, LLMConfig},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

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
            description: "Echo tool for agent loop tests".to_string(),
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

fn build_incoming(content: &str, external_thread_id: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some(external_thread_id.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread(external_thread_id: &str) -> Thread {
    let locator = ThreadContextLocator::new(
        Some("session_1".to_string()),
        "feishu",
        "ou_xxx",
        external_thread_id,
        Uuid::new_v4().to_string(),
    );
    let mut thread = Thread::new(locator, Utc::now());
    let _ = thread.ensure_system_prompt_snapshot("system", Utc::now());
    thread
}

fn build_event_sender(
    incoming: &IncomingMessage,
    thread_context: &Thread,
) -> (AgentEventSender, mpsc::Receiver<AgentDispatchEvent>) {
    let (tx, rx) = mpsc::channel(16);
    (
        AgentEventSender::from_incoming_and_locator(tx, incoming, &thread_context.locator),
        rx,
    )
}

#[tokio::test]
async fn run_v1_emits_text_output_and_returns_commit_messages() {
    // 测试场景: 主入口只消费 Thread + incoming，loop 负责事件化最终文本并返回待提交消息。
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(MockLLMProvider::new("loop-reply")),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_loop_text");
    let incoming = build_incoming("hello", "thread_loop_text");
    let (event_tx, mut event_rx) = build_event_sender(&incoming, &thread_context);

    let output = loop_runner
        .run_v1(event_tx, &incoming, thread_context.clone())
        .await
        .expect("loop should succeed");

    let event = event_rx.recv().await.expect("text event should be emitted");
    assert_eq!(event.kind, AgentLoopEventKind::TextOutput);
    assert_eq!(event.content, "loop-reply");
    assert_eq!(output.reply, "loop-reply");
    assert_eq!(output.commit_messages.len(), 1);
    assert_eq!(output.commit_messages[0].content, "loop-reply");
    assert_eq!(
        output.thread_context.system_prefix_messages()[0].content,
        "system"
    );
    assert!(output.thread_context.load_messages().is_empty());
}

#[tokio::test]
async fn run_v1_executes_tool_calls_and_records_tool_events() {
    // 测试场景: loop 必须把 assistant/tool 消息加入 commit_messages，并记录对应 tool event。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(EchoTool))
        .await
        .expect("echo tool should register");
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "我先查一下",
                Utc::now(),
            )),
            tool_calls: vec![LLMToolCall {
                id: "call_demo_1".to_string(),
                name: "demo__echo".to_string(),
                arguments: json!({}),
            }],
        },
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "done",
                Utc::now(),
            )),
            tool_calls: Vec::new(),
        },
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_loop_tool");
    let incoming = build_incoming("run tool", "thread_loop_tool");
    let (event_tx, mut event_rx) = build_event_sender(&incoming, &thread_context);

    let output = loop_runner
        .run_v1(event_tx, &incoming, thread_context)
        .await
        .expect("loop should succeed");

    let mut kinds = Vec::new();
    while let Ok(event) =
        tokio::time::timeout(std::time::Duration::from_millis(20), event_rx.recv()).await
    {
        let Some(event) = event else {
            break;
        };
        kinds.push(event.kind);
    }

    assert!(kinds.contains(&AgentLoopEventKind::TextOutput));
    assert!(kinds.contains(&AgentLoopEventKind::ToolCall));
    assert!(kinds.contains(&AgentLoopEventKind::ToolResult));
    assert_eq!(output.reply, "done");
    assert_eq!(output.commit_messages.len(), 3);
    assert_eq!(output.commit_messages[0].content, "我先查一下");
    assert_eq!(
        output.commit_messages[1].tool_call_id.as_deref(),
        Some("call_demo_1")
    );
    assert_eq!(output.commit_messages[2].content, "done");
    assert_eq!(output.tool_events.len(), 1);
    assert_eq!(
        output.tool_events[0].tool_name.as_deref(),
        Some("demo__echo")
    );
}

#[tokio::test]
async fn run_v1_tool_requested_compact_replaces_non_system_history() {
    // 测试场景: compact tool 应只压缩非 system 消息，并通过 compact event 报告结果。
    let compact_config: AgentCompactConfig = serde_json::from_value(json!({
        "enabled": true,
        "auto_compact": false,
        "runtime_threshold_ratio": 1.0,
        "tool_visible_threshold_ratio": 1.0,
        "mock_compacted_assistant": "任务已压缩"
    }))
    .expect("compact config should parse");
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            message: Some(ChatMessage::new(ChatMessageRole::Assistant, "", Utc::now())),
            tool_calls: vec![LLMToolCall {
                id: "call_compact_1".to_string(),
                name: "compact".to_string(),
                arguments: json!({}),
            }],
        },
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "after compact",
                Utc::now(),
            )),
            tool_calls: Vec::new(),
        },
    ]);
    let loop_runner = AgentLoop::with_compact_config_and_system_prompt(
        Arc::new(provider),
        AgentRuntime::new(),
        LLMConfig::default(),
        compact_config,
        None::<String>,
    );
    let mut thread_context = build_thread("thread_loop_compact");
    let now = Utc::now();
    thread_context.store_turn(
        Some("msg_history".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "old question", now),
            ChatMessage::new(ChatMessageRole::Assistant, "old answer", now),
        ],
        now,
        now,
    );
    let incoming = build_incoming("continue", "thread_loop_compact");
    let (event_tx, mut event_rx) = build_event_sender(&incoming, &thread_context);

    let output = loop_runner
        .run_v1(event_tx, &incoming, thread_context)
        .await
        .expect("loop should succeed");

    let mut saw_compact = false;
    while let Ok(event) =
        tokio::time::timeout(std::time::Duration::from_millis(20), event_rx.recv()).await
    {
        let Some(event) = event else {
            break;
        };
        if event.kind == AgentLoopEventKind::Compact {
            saw_compact = true;
        }
    }

    assert!(saw_compact);
    assert!(!output.persist_incoming_user);
    assert_eq!(output.reply, "after compact");
    assert_eq!(
        output.thread_context.system_prefix_messages()[0].content,
        "system"
    );
    assert_eq!(output.thread_context.load_messages().len(), 2);
    assert!(
        output.thread_context.load_messages()[0]
            .content
            .contains("这是压缩后的上下文")
    );
    assert_eq!(output.thread_context.load_messages()[1].content, "继续");
}
