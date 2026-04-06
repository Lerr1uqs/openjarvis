use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEventKind, AgentRuntime,
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        agent_loop::{
            AgentLoopUTLLMResponseSnapshot, AgentLoopUTLoopState, AgentLoopUTProber,
            AgentLoopUTRequestSnapshot, AgentLoopUTToolCallSnapshot, AgentLoopUTToolResultSnapshot,
            TOOL_EVENT_PREVIEW_MAX_CHARS, UTProbe, truncate_tool_log_preview,
            truncate_tool_message,
        },
        empty_tool_input_schema,
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

struct LongResultTool;

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

#[async_trait]
impl ToolHandler for LongResultTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__long_echo".to_string(),
            description: "Long tool result for event truncation tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "R".repeat(128),
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

#[derive(Default)]
struct RecordingUTProbe {
    loop_begin: Vec<AgentLoopUTLoopState>,
    request_prepared: Vec<AgentLoopUTRequestSnapshot>,
    llm_responses: Vec<AgentLoopUTLLMResponseSnapshot>,
    tool_calls: Vec<AgentLoopUTToolCallSnapshot>,
    tool_results: Vec<AgentLoopUTToolResultSnapshot>,
    loop_end: Vec<AgentLoopUTLoopState>,
}

impl AgentLoopUTProber for RecordingUTProbe {
    fn on_loop_begin(&mut self, state: &AgentLoopUTLoopState) {
        self.loop_begin.push(state.clone());
    }

    fn on_request_prepared(&mut self, snapshot: &AgentLoopUTRequestSnapshot) {
        self.request_prepared.push(snapshot.clone());
    }

    fn on_llm_response(&mut self, snapshot: &AgentLoopUTLLMResponseSnapshot) {
        self.llm_responses.push(snapshot.clone());
    }

    fn on_tool_call_start(&mut self, snapshot: &AgentLoopUTToolCallSnapshot) {
        self.tool_calls.push(snapshot.clone());
    }

    fn on_tool_result(&mut self, snapshot: &AgentLoopUTToolResultSnapshot) {
        self.tool_results.push(snapshot.clone());
    }

    fn on_loop_end(&mut self, state: &AgentLoopUTLoopState) {
        self.loop_end.push(state.clone());
    }
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
async fn run_v1_with_ut_probe_captures_intermediate_loop_state() {
    // 测试场景: UTProbe 需要在不改动生产语义的前提下，暴露 loop 中间态供集成测试断言。
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
                id: "call_demo_probe_1".to_string(),
                name: "demo__echo".to_string(),
                arguments: json!({"query": "demo"}),
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
    let thread_context = build_thread("thread_loop_ut_probe");
    let incoming = build_incoming("run tool", "thread_loop_ut_probe");
    let (event_tx, _event_rx) = build_event_sender(&incoming, &thread_context);
    let mut probe = RecordingUTProbe::default();

    let output = loop_runner
        .run_v1_with_ut_probe(
            event_tx,
            &incoming,
            thread_context,
            Some(&mut probe as UTProbe<'_>),
        )
        .await
        .expect("loop should succeed");

    assert_eq!(probe.loop_begin.len(), 2);
    assert_eq!(probe.request_prepared.len(), 2);
    assert_eq!(probe.llm_responses.len(), 2);
    assert_eq!(probe.tool_calls.len(), 1);
    assert_eq!(probe.tool_results.len(), 1);
    assert_eq!(probe.loop_end.len(), 2);

    assert_eq!(probe.loop_begin[0].iteration, 0);
    assert!(probe.loop_begin[0].thread_messages.is_empty());
    assert_eq!(probe.loop_begin[0].live_chat_messages.len(), 1);
    assert_eq!(
        probe.loop_begin[0].live_chat_messages[0].content,
        "run tool"
    );
    assert!(probe.loop_begin[0].commit_messages.is_empty());

    assert_eq!(probe.request_prepared[0].iteration, 0);
    assert_eq!(
        probe.request_prepared[0]
            .messages
            .last()
            .expect("user message should exist")
            .content,
        "run tool"
    );
    assert!(!probe.request_prepared[0].tools.is_empty());

    assert_eq!(probe.llm_responses[0].iteration, 0);
    assert_eq!(probe.llm_responses[0].tool_calls.len(), 1);
    assert_eq!(
        probe.llm_responses[0]
            .message
            .as_ref()
            .expect("assistant text should exist")
            .content,
        "我先查一下"
    );

    assert_eq!(probe.tool_calls[0].iteration, 0);
    assert_eq!(probe.tool_calls[0].tool_call_id, "call_demo_probe_1");
    assert_eq!(probe.tool_calls[0].request.name, "demo__echo");

    assert_eq!(probe.tool_results[0].iteration, 0);
    assert_eq!(probe.tool_results[0].tool_call_id, "call_demo_probe_1");
    assert_eq!(probe.tool_results[0].result.content, "echo-result");
    assert!(!probe.tool_results[0].result.is_error);

    assert_eq!(probe.loop_end[0].iteration, 0);
    assert_eq!(probe.loop_end[0].commit_messages.len(), 2);
    assert_eq!(probe.loop_end[0].commit_messages[0].content, "我先查一下");
    assert_eq!(probe.loop_end[0].commit_messages[1].content, "echo-result");
    assert_eq!(
        probe.loop_end[0].commit_messages[1].tool_call_id.as_deref(),
        Some("call_demo_probe_1")
    );

    assert_eq!(probe.loop_begin[1].iteration, 1);
    assert_eq!(probe.loop_begin[1].live_chat_messages.len(), 3);
    assert_eq!(
        probe.loop_begin[1].live_chat_messages[1].content,
        "我先查一下"
    );
    assert_eq!(
        probe.loop_begin[1].live_chat_messages[1].tool_calls.len(),
        1
    );
    assert_eq!(
        probe.loop_begin[1].live_chat_messages[2].content,
        "echo-result"
    );

    assert_eq!(probe.llm_responses[1].iteration, 1);
    assert_eq!(probe.llm_responses[1].tool_calls.len(), 0);
    assert_eq!(
        probe.llm_responses[1]
            .message
            .as_ref()
            .expect("final assistant text should exist")
            .content,
        "done"
    );

    assert_eq!(probe.loop_end[1].iteration, 1);
    assert_eq!(
        probe.loop_end[1]
            .commit_messages
            .last()
            .expect("final assistant message should exist")
            .content,
        "done"
    );
    assert!(probe.loop_end[1].persist_incoming_user);
    assert_eq!(output.reply, "done");
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
async fn run_v1_truncates_tool_event_content_but_keeps_full_tool_result_history() {
    // 测试场景: tool_call/tool_result 发给 router 的事件内容必须截断，但写入历史供模型继续推理的 tool result 必须保留完整内容。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(LongResultTool))
        .await
        .expect("long result tool should register");
    let long_arguments = json!({
        "path": "X".repeat(128),
    });
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "开始执行",
                Utc::now(),
            )),
            tool_calls: vec![LLMToolCall {
                id: "call_long_1".to_string(),
                name: "demo__long_echo".to_string(),
                arguments: long_arguments.clone(),
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
    let thread_context = build_thread("thread_loop_tool_truncate");
    let incoming = build_incoming("run tool", "thread_loop_tool_truncate");
    let (event_tx, mut event_rx) = build_event_sender(&incoming, &thread_context);

    let output = loop_runner
        .run_v1(event_tx, &incoming, thread_context)
        .await
        .expect("loop should succeed");

    let mut events = Vec::new();
    while let Ok(event) =
        tokio::time::timeout(std::time::Duration::from_millis(20), event_rx.recv()).await
    {
        let Some(event) = event else {
            break;
        };
        events.push(event);
    }

    let tool_call_event = events
        .iter()
        .find(|event| event.kind == AgentLoopEventKind::ToolCall)
        .expect("tool_call event should be emitted");
    let tool_result_event = events
        .iter()
        .find(|event| event.kind == AgentLoopEventKind::ToolResult)
        .expect("tool_result event should be emitted");

    assert_eq!(
        tool_call_event.content,
        format!(
            "[openjarvis][tool_call] demo__long_echo {}",
            truncate_tool_message(&long_arguments.to_string(), TOOL_EVENT_PREVIEW_MAX_CHARS)
        )
    );
    assert_eq!(
        tool_result_event.content,
        format!(
            "[openjarvis][tool_result] {}",
            truncate_tool_message(&"R".repeat(128), TOOL_EVENT_PREVIEW_MAX_CHARS)
        )
    );
    assert_eq!(output.commit_messages.len(), 3);
    assert_eq!(
        output.commit_messages[1].tool_call_id.as_deref(),
        Some("call_long_1")
    );
    assert_eq!(output.commit_messages[1].content, "R".repeat(128));
    assert_eq!(output.reply, "done");
}

#[test]
fn tool_log_preview_truncates_long_content() {
    // 测试场景: tool 日志预览必须对超长内容进行截断，避免 arguments/result 把日志撑爆。
    let preview = truncate_tool_log_preview(&"A".repeat(700), 32);

    assert!(preview.starts_with("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    assert!(preview.contains("...(truncated, total_chars=700)"));
    assert!(preview.len() < 100);
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
