use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentEventSender, AgentLoop, AgentLoopEventKind, AgentLoopOutput, AgentRuntime,
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        agent_loop::{
            AgentLoopUTCompactSnapshot, AgentLoopUTLLMResponseSnapshot, AgentLoopUTLoopState,
            AgentLoopUTProber, AgentLoopUTRequestSnapshot, AgentLoopUTToolCallSnapshot,
            AgentLoopUTToolResultSnapshot, TOOL_EVENT_PREVIEW_MAX_CHARS, UTProbe,
            truncate_tool_log_preview, truncate_tool_message,
        },
        empty_tool_input_schema,
    },
    config::{AgentCompactConfig, LLMConfig},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    thread::{Thread, ThreadContextLocator, ThreadFinalizedTurnStatus},
};
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;
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

struct FailingLLMProvider;

#[async_trait]
impl LLMProvider for FailingLLMProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        Err(anyhow!(
            "upstream llm transport failed: connection reset by peer"
        ))
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
            content: "R".repeat(512),
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

fn build_thread_with_system_messages(external_thread_id: &str, system_count: usize) -> Thread {
    let locator = ThreadContextLocator::new(
        Some("session_1".to_string()),
        "feishu",
        "ou_xxx",
        external_thread_id,
        Uuid::new_v4().to_string(),
    );
    let mut thread = Thread::new(locator, Utc::now());
    let system_messages = (0..system_count)
        .map(|index| {
            ChatMessage::new(
                ChatMessageRole::System,
                format!("system-slot-{index} {}", "S".repeat(24)),
                Utc::now(),
            )
        })
        .collect::<Vec<_>>();
    assert!(thread.ensure_system_prefix_messages(&system_messages));
    thread
}

fn build_event_sender(incoming: &IncomingMessage, thread_context: &Thread) -> AgentEventSender {
    AgentEventSender::from_incoming_and_locator(incoming, &thread_context.locator)
}

fn only_turn(output: &AgentLoopOutput) -> &openjarvis::agent::CompletedAgentTurn {
    assert_eq!(output.turns.len(), 1);
    &output.turns[0]
}

fn last_turn(output: &AgentLoopOutput) -> &openjarvis::agent::CompletedAgentTurn {
    output
        .turns
        .last()
        .expect("agent loop should finalize at least one turn")
}

fn flattened_dispatch_kinds(output: &AgentLoopOutput) -> Vec<AgentLoopEventKind> {
    output
        .turns
        .iter()
        .flat_map(|turn| turn.dispatch_batch.iter().map(|event| event.kind.clone()))
        .collect()
}

#[derive(Default)]
struct RecordingUTProbe {
    loop_begin: Vec<AgentLoopUTLoopState>,
    request_prepared: Vec<AgentLoopUTRequestSnapshot>,
    llm_responses: Vec<AgentLoopUTLLMResponseSnapshot>,
    tool_calls: Vec<AgentLoopUTToolCallSnapshot>,
    tool_results: Vec<AgentLoopUTToolResultSnapshot>,
    compacts: Vec<AgentLoopUTCompactSnapshot>,
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

    fn on_compact(&mut self, snapshot: &AgentLoopUTCompactSnapshot) {
        self.compacts.push(snapshot.clone());
    }

    fn on_loop_end(&mut self, state: &AgentLoopUTLoopState) {
        self.loop_end.push(state.clone());
    }
}

#[tokio::test]
async fn run_v1_returns_finalized_turn_batch_for_plain_text_reply() {
    // 测试场景: loop 只返回 finalized turn batch；最终文本和 thread snapshot 来自同一个 turn 边界。
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(MockLLMProvider::new("loop-reply")),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_loop_text");
    let incoming = build_incoming("hello", "thread_loop_text");

    let output = loop_runner
        .run_v1(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context.clone(),
        )
        .await
        .expect("loop should succeed");

    let turn = only_turn(&output);
    assert_eq!(output.reply, "loop-reply");
    assert_eq!(turn.dispatch_batch.len(), 1);
    assert_eq!(turn.dispatch_batch[0].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(turn.dispatch_batch[0].content, "loop-reply");
    assert_eq!(
        turn.turn
            .snapshot
            .load_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec!["hello".to_string(), "loop-reply".to_string()]
    );
}

#[tokio::test]
async fn run_v1_drops_failed_turn_contents_when_llm_generate_errors() {
    // 测试场景: `llm.generate()` 发生异常时，本轮 turn 内容必须整体丢弃；
    // finalized snapshot 只能保留失败前已有的 persisted history。
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(FailingLLMProvider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let mut thread_context = build_thread("thread_loop_failure");
    let now = Utc::now();
    thread_context.store_turn(
        Some("msg_seed".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "persisted history",
            now,
        )],
        now,
        now,
    );
    let incoming = build_incoming("hello", "thread_loop_failure");
    let mut probe = RecordingUTProbe::default();

    let output = loop_runner
        .run_v1_with_ut_probe(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
            Some(&mut probe as UTProbe<'_>),
        )
        .await
        .expect("unexpected llm failure should still finalize one failed turn");

    let turn = only_turn(&output);
    assert!(matches!(
        turn.turn.status,
        ThreadFinalizedTurnStatus::Failed { .. }
    ));
    assert_eq!(
        turn.turn
            .snapshot
            .load_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec!["persisted history".to_string()]
    );
    assert!(
        turn.dispatch_batch[0]
            .content
            .contains("[openjarvis][agent_error]")
    );
    assert_eq!(probe.request_prepared.len(), 1);
    assert!(probe.llm_responses.is_empty());
    assert_eq!(probe.loop_end.len(), 1);
    assert_eq!(probe.loop_end[0].current_turn_working_messages.len(), 1);
}

#[tokio::test]
async fn run_v1_with_ut_probe_exposes_thread_owned_turn_state() {
    // 测试场景: 中间态探针只能看到 Thread 自身的 request view / working set，不再看到 loop-local message 集合。
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
    let mut probe = RecordingUTProbe::default();

    let output = loop_runner
        .run_v1_with_ut_probe(
            build_event_sender(&incoming, &thread_context),
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
    assert!(probe.loop_begin[0].current_turn_working_messages.is_empty());
    assert_eq!(
        probe.request_prepared[0]
            .messages
            .last()
            .expect("first request should include the user input")
            .content,
        "run tool"
    );
    assert_eq!(probe.loop_end[0].turn_events.len(), 3);
    assert_eq!(
        probe.loop_end[0]
            .current_turn_working_messages
            .last()
            .and_then(|message| message.tool_call_id.as_deref()),
        Some("call_demo_probe_1")
    );
    assert_eq!(output.turns.len(), 2);
    assert_eq!(flattened_dispatch_kinds(&output).len(), 4);
    assert_eq!(output.reply, "done");
}

#[tokio::test]
async fn run_v1_batches_multiple_tool_calls_and_probes_each_iteration() {
    // 测试场景: 单次 LLM 响应返回多个 tool call 时，
    // loop 必须按顺序执行并把每一轮迭代与每个 tool call 都暴露给 UT probe。
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
                "我会连续调用两个工具",
                Utc::now(),
            )),
            tool_calls: vec![
                LLMToolCall {
                    id: "call_demo_batch_1".to_string(),
                    name: "demo__echo".to_string(),
                    arguments: json!({"query": "first"}),
                },
                LLMToolCall {
                    id: "call_demo_batch_2".to_string(),
                    name: "demo__echo".to_string(),
                    arguments: json!({"query": "second"}),
                },
            ],
        },
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "batch done",
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
    let thread_context = build_thread("thread_loop_batch_tools");
    let incoming = build_incoming("run batch tools", "thread_loop_batch_tools");
    let mut probe = RecordingUTProbe::default();

    let output = loop_runner
        .run_v1_with_ut_probe(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
            Some(&mut probe as UTProbe<'_>),
        )
        .await
        .expect("loop should succeed");

    let first_turn = &output.turns[0];
    let final_turn = last_turn(&output);
    assert_eq!(
        probe
            .loop_begin
            .iter()
            .map(|state| state.iteration)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        probe
            .request_prepared
            .iter()
            .map(|snapshot| snapshot.iteration)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        probe
            .llm_responses
            .iter()
            .map(|snapshot| snapshot.iteration)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        probe
            .tool_calls
            .iter()
            .map(|snapshot| (snapshot.iteration, snapshot.tool_call_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "call_demo_batch_1"), (0, "call_demo_batch_2")]
    );
    assert_eq!(
        probe
            .tool_results
            .iter()
            .map(|snapshot| (snapshot.iteration, snapshot.tool_call_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "call_demo_batch_1"), (0, "call_demo_batch_2")]
    );
    assert_eq!(probe.loop_end[0].turn_events.len(), 5);
    assert_eq!(output.turns.len(), 2);
    assert_eq!(
        first_turn.turn.snapshot.load_messages()[1].tool_calls.len(),
        2
    );
    assert_eq!(
        flattened_dispatch_kinds(&output),
        vec![
            AgentLoopEventKind::TextOutput,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::TextOutput,
        ]
    );
    assert_eq!(final_turn.turn.reply, "batch done");
}

#[tokio::test]
async fn run_v1_executes_tool_calls_and_persists_tool_events_in_snapshot() {
    // 测试场景: tool call/tool result/final reply 都先进入 Thread 当前 turn，finalize 后再一起外发和落盘。
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
    let thread_context = build_thread("thread_loop_tool");
    let incoming = build_incoming("run tool", "thread_loop_tool");

    let output = loop_runner
        .run_v1(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
        )
        .await
        .expect("loop should succeed");

    let final_turn = last_turn(&output);
    assert_eq!(
        final_turn
            .turn
            .snapshot
            .load_messages()
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::User,
            ChatMessageRole::Assistant,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
    assert_eq!(final_turn.turn.snapshot.load_tool_events().len(), 1);
    assert_eq!(
        final_turn.turn.snapshot.load_tool_events()[0]
            .tool_name
            .as_deref(),
        Some("demo__echo")
    );
    assert_eq!(
        flattened_dispatch_kinds(&output),
        vec![
            AgentLoopEventKind::TextOutput,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::TextOutput,
        ]
    );
}

#[tokio::test]
async fn run_v1_truncates_tool_events_but_keeps_full_tool_result_history() {
    // 测试场景: 对外事件要截断长 tool 文本，但 Thread finalized snapshot 中必须保留完整 tool result。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(LongResultTool))
        .await
        .expect("long tool should register");
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "准备调用长工具",
                Utc::now(),
            )),
            tool_calls: vec![LLMToolCall {
                id: "call_demo_long_1".to_string(),
                name: "demo__long_echo".to_string(),
                arguments: json!({"query": "demo"}),
            }],
        },
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "long done",
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
    let thread_context = build_thread("thread_long_tool");
    let incoming = build_incoming("run long tool", "thread_long_tool");

    let output = loop_runner
        .run_v1(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
        )
        .await
        .expect("loop should succeed");

    let tool_result_event = output
        .turns
        .iter()
        .flat_map(|turn| turn.dispatch_batch.iter())
        .find(|event| event.kind == AgentLoopEventKind::ToolResult)
        .expect("tool result event should exist");
    assert!(tool_result_event.content.contains("...(truncated"));
    assert_eq!(
        last_turn(&output).turn.snapshot.load_messages()[2].content,
        "R".repeat(512)
    );
}

#[test]
fn tool_log_preview_truncates_long_content() {
    // 测试场景: 日志和 channel 预览都应保留统一的截断格式，避免测试依赖两套逻辑。
    assert_eq!(
        truncate_tool_message("123456", 4),
        "1234...(truncated, total_chars=6)"
    );
    assert_eq!(
        truncate_tool_log_preview("abcdef", 3),
        "abc...(truncated, total_chars=6)"
    );
    assert_eq!(TOOL_EVENT_PREVIEW_MAX_CHARS, 300);
}

#[tokio::test]
async fn run_v1_tool_requested_compact_replaces_thread_owned_active_view() {
    // 测试场景: compact tool 直接改写 Thread active non-system view，Router/Session 不再额外跳过用户消息。
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
            message: None,
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
        Some("system".to_string()),
    );
    let mut thread_context = build_thread("thread_loop_compact");
    let now = Utc::now();
    thread_context.store_turn(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::User, "old request", now),
            ChatMessage::new(ChatMessageRole::Assistant, "old reply", now),
        ],
        now,
        now,
    );
    let incoming = build_incoming("continue", "thread_loop_compact");

    let output = loop_runner
        .run_v1(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
        )
        .await
        .expect("loop should succeed");

    let first_turn = &output.turns[0];
    let final_turn = last_turn(&output);
    assert_eq!(output.reply, "after compact");
    assert_eq!(
        flattened_dispatch_kinds(&output),
        vec![
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::Compact,
            AgentLoopEventKind::TextOutput,
        ]
    );
    assert_eq!(first_turn.turn.snapshot.load_messages().len(), 3);
    assert_eq!(
        first_turn.turn.snapshot.load_messages()[0].content,
        "这是压缩后的上下文，请基于这些信息继续当前任务：\n任务已压缩"
    );
    assert_eq!(first_turn.turn.snapshot.load_messages()[1].content, "继续");
    assert_eq!(
        first_turn.turn.snapshot.load_messages()[2].content,
        "compact completed: compacted 3 messages from current chat history"
    );
    assert_eq!(final_turn.turn.snapshot.load_messages().len(), 4);
    assert_eq!(final_turn.turn.snapshot.load_messages()[1].content, "继续");
    assert_eq!(
        final_turn.turn.snapshot.load_messages()[3].content,
        "after compact"
    );
}

#[tokio::test]
#[ignore = "temporarily disabled because this auto-compact integration case can destabilize the host during cargo test"]
async fn run_v1_auto_compact_feature_exposes_compact_tool_and_keeps_system_prefix() {
    // 测试场景: 开启 auto-compact feature 且上下文逼近阈值时，
    // 模型应能看到 compact 工具并触发压缩；压缩后 system prefix 仍必须完整保留在开头。
    let compact_config: AgentCompactConfig = serde_json::from_value(json!({
        "enabled": true,
        "auto_compact": true,
        "runtime_threshold_ratio": 0.95,
        "tool_visible_threshold_ratio": 0.5,
        "mock_compacted_assistant": "自动压缩摘要"
    }))
    .expect("compact config should parse");
    let llm_config = LLMConfig {
        context_window_tokens: Some(320),
        max_output_tokens: Some(16),
        ..LLMConfig::default()
    };
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            message: None,
            tool_calls: vec![LLMToolCall {
                id: "call_auto_compact_1".to_string(),
                name: "compact".to_string(),
                arguments: json!({}),
            }],
        },
        LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "after auto compact",
                Utc::now(),
            )),
            tool_calls: Vec::new(),
        },
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        AgentRuntime::new(),
        llm_config,
        compact_config,
    );
    let mut thread_context = build_thread_with_system_messages("thread_loop_auto_compact", 5);
    let now = Utc::now();
    let history = (0..5)
        .flat_map(|index| {
            [
                ChatMessage::new(
                    ChatMessageRole::User,
                    format!("user-history-{index} {}", "U".repeat(20)),
                    now,
                ),
                ChatMessage::new(
                    ChatMessageRole::Assistant,
                    format!("assistant-history-{index} {}", "A".repeat(20)),
                    now,
                ),
            ]
        })
        .collect::<Vec<_>>();
    thread_context.store_turn(None, history, now, now);
    thread_context.enable_auto_compact();
    let incoming = build_incoming("continue", "thread_loop_auto_compact");
    let mut probe = RecordingUTProbe::default();

    let output = loop_runner
        .run_v1_with_ut_probe(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
            Some(&mut probe as UTProbe<'_>),
        )
        .await
        .expect("auto-compact loop should succeed");

    assert!(probe.request_prepared[0].budget_report.utilization_ratio >= 0.5);
    assert!(
        probe.request_prepared[0]
            .tools
            .iter()
            .any(|tool| tool.name == "compact")
    );
    assert_eq!(probe.compacts.len(), 1);
    assert!(probe.compacts[0].requested_by_model);
    assert!(!probe.compacts[0].is_error);
    assert_eq!(probe.compacts[0].active_non_system_messages.len(), 2);
    assert!(probe.compacts[0].current_turn_working_messages.is_empty());
    assert_eq!(
        last_turn(&output)
            .turn
            .snapshot
            .system_prefix_messages()
            .len(),
        5
    );
    assert_eq!(last_turn(&output).turn.snapshot.messages().len(), 9);
    assert_eq!(
        last_turn(&output)
            .turn
            .snapshot
            .messages()
            .iter()
            .take(5)
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::System,
            ChatMessageRole::System,
            ChatMessageRole::System,
            ChatMessageRole::System,
            ChatMessageRole::System,
        ]
    );
    assert_eq!(
        last_turn(&output)
            .turn
            .snapshot
            .load_messages()
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::Assistant,
            ChatMessageRole::User,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.load_messages()[0].content,
        "这是压缩后的上下文，请基于这些信息继续当前任务：\n自动压缩摘要"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.load_messages()[1].content,
        "继续"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.load_messages()[2].content,
        "compact completed: compacted 11 messages from current chat history"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.load_messages()[3].content,
        "after auto compact"
    );
}
