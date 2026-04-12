use super::support::ThreadTestExt;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEventKind, AgentLoopOutput,
        AgentRuntime, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        agent_loop::{
            AgentCommittedMessageHandler, AgentLoopUTCompactSnapshot,
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
struct WeatherTool;
struct RouteTool;
struct HotelTool;

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

#[async_trait]
impl ToolHandler for WeatherTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__weather".to_string(),
            description: "Weather tool for responses-style loop tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "{\"forecast\":\"sunny\",\"temp\":23}".to_string(),
            metadata: json!({}),
            is_error: false,
        })
    }
}

#[async_trait]
impl ToolHandler for RouteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__route".to_string(),
            description: "Route planner tool for responses-style loop tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "{\"trains\":[\"G7311\"]}".to_string(),
            metadata: json!({}),
            is_error: false,
        })
    }
}

#[async_trait]
impl ToolHandler for HotelTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__hotel".to_string(),
            description: "Hotel search tool for responses-style loop tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "{\"hotels\":[\"West Lake Hotel\"]}".to_string(),
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
    thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system",
        Utc::now(),
    )]);
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
    thread.seed_persisted_messages(system_messages);
    thread
}

fn build_event_sender(incoming: &IncomingMessage, thread_context: &Thread) -> AgentEventSender {
    AgentEventSender::from_incoming_and_locator(incoming, &thread_context.locator)
}

fn scripted_tool_call(id: &str, name: &str, arguments: serde_json::Value) -> LLMToolCall {
    LLMToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
        provider_item_id: None,
    }
}

fn scripted_llm_response(
    message: Option<ChatMessage>,
    tool_calls: Vec<LLMToolCall>,
) -> LLMResponse {
    let mut items = Vec::new();
    if let Some(message) = message {
        items.push(message);
    }
    items.extend(tool_calls.into_iter().map(|tool_call| {
        ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now()).with_tool_calls(vec![tool_call])
    }));
    LLMResponse { items }
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

fn collected_event_kinds(events: &[AgentDispatchEvent]) -> Vec<AgentLoopEventKind> {
    events.iter().map(|event| event.kind.clone()).collect()
}

fn collected_event_contents(events: &[AgentDispatchEvent]) -> Vec<String> {
    events.iter().map(|event| event.content.clone()).collect()
}

async fn run_locked_thread_with_recorded_events(
    loop_runner: &AgentLoop,
    incoming: &IncomingMessage,
    mut thread_context: Thread,
) -> (AgentLoopOutput, Vec<AgentDispatchEvent>) {
    let mut handler = RecordingDispatchHandler::default();
    let output = loop_runner
        .run_locked_thread(
            build_event_sender(incoming, &thread_context),
            incoming,
            &mut thread_context,
            &mut handler,
        )
        .await
        .expect("loop should succeed");
    (output, handler.events)
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

#[derive(Default)]
struct RecordingDispatchHandler {
    events: Vec<AgentDispatchEvent>,
}

#[async_trait]
impl AgentCommittedMessageHandler for RecordingDispatchHandler {
    async fn on_committed_message(
        &mut self,
        _thread_context: &mut Thread,
        _message: ChatMessage,
        dispatch_events: Vec<AgentDispatchEvent>,
    ) -> Result<()> {
        self.events.extend(dispatch_events);
        Ok(())
    }
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

    let (output, events) =
        run_locked_thread_with_recorded_events(&loop_runner, &incoming, thread_context).await;

    let turn = only_turn(&output);
    assert_eq!(output.reply, "loop-reply");
    assert_eq!(
        collected_event_kinds(&events),
        vec![AgentLoopEventKind::TextOutput]
    );
    assert_eq!(
        collected_event_contents(&events),
        vec!["loop-reply".to_string()]
    );
    assert_eq!(
        turn.turn
            .snapshot
            .non_system_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec!["hello".to_string(), "loop-reply".to_string()]
    );
}

#[tokio::test]
async fn run_locked_thread_emits_committed_events_in_message_order() {
    // 测试场景: 同一 turn 内的文本、tool call、tool result、最终文本必须按单条 committed message 顺序即时发送。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(EchoTool))
        .await
        .expect("echo tool should register");
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(ScriptedLLMProvider::new(vec![
            scripted_llm_response(
                Some(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "thinking",
                    Utc::now(),
                )),
                vec![scripted_tool_call("call_1", "demo__echo", json!({}))],
            ),
            scripted_llm_response(
                Some(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "done",
                    Utc::now(),
                )),
                Vec::new(),
            ),
        ])),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let incoming = build_incoming("hello", "thread_dispatch_seq");
    let mut thread_context = build_thread("thread_dispatch_seq");
    let mut handler = RecordingDispatchHandler::default();

    let output = loop_runner
        .run_locked_thread(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            &mut thread_context,
            &mut handler,
        )
        .await
        .expect("loop should succeed");

    assert_eq!(output.reply, "done");
    assert_eq!(
        handler
            .events
            .iter()
            .map(|event| event.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            AgentLoopEventKind::TextOutput,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::TextOutput,
        ]
    );
    assert_eq!(
        handler
            .events
            .iter()
            .map(|event| event.content.as_str())
            .collect::<Vec<_>>(),
        vec![
            "thinking",
            "[openjarvis][tool_call] demo__echo {}",
            "[openjarvis][tool_result] echo-result",
            "done",
        ]
    );
    assert!(handler.events[0].reply_to_source);
    assert!(
        handler.events[1..]
            .iter()
            .all(|event| !event.reply_to_source)
    );
    assert!(
        handler
            .events
            .windows(2)
            .all(|window| window[0].source_message_id == window[1].source_message_id)
    );
}

#[tokio::test]
async fn run_v1_drops_failed_turn_contents_when_llm_generate_errors() {
    // 测试场景: `llm.generate()` 发生异常时，已提交消息不能回滚，失败消息必须继续附加到正式历史。
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(FailingLLMProvider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let mut thread_context = build_thread("thread_loop_failure");
    let now = Utc::now();
    thread_context.commit_test_turn(
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
            .non_system_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "persisted history".to_string(),
            "hello".to_string(),
            turn.turn.reply.clone(),
        ]
    );
    assert!(turn.turn.reply.contains("[openjarvis][agent_error]"));
    assert_eq!(probe.request_prepared.len(), 1);
    assert!(probe.llm_responses.is_empty());
    assert_eq!(probe.loop_end.len(), 1);
    assert_eq!(probe.loop_end[0].request_messages.len(), 3);
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
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "我先查一下",
                Utc::now(),
            )),
            vec![scripted_tool_call(
                "call_demo_probe_1",
                "demo__echo",
                json!({"query": "demo"}),
            )],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "done",
                Utc::now(),
            )),
            Vec::new(),
        ),
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
    assert_eq!(probe.loop_begin[0].request_messages.len(), 2);
    assert_eq!(
        probe.request_prepared[0]
            .messages
            .last()
            .expect("first request should include the user input")
            .content,
        "run tool"
    );
    assert!(probe.loop_end[0].turn_events.is_empty());
    assert_eq!(
        probe.loop_end[0]
            .request_messages
            .last()
            .and_then(|message| message.tool_call_id.as_deref()),
        Some("call_demo_probe_1")
    );
    assert_eq!(output.turns.len(), 1);
    assert_eq!(output.turns.len(), 1);
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
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "我会连续调用两个工具",
                Utc::now(),
            )),
            vec![
                scripted_tool_call("call_demo_batch_1", "demo__echo", json!({"query": "first"})),
                scripted_tool_call(
                    "call_demo_batch_2",
                    "demo__echo",
                    json!({"query": "second"}),
                ),
            ],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "batch done",
                Utc::now(),
            )),
            Vec::new(),
        ),
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
    assert!(probe.loop_end[0].turn_events.is_empty());
    assert_eq!(output.turns.len(), 1);
    assert_eq!(
        first_turn
            .turn
            .snapshot
            .non_system_messages()
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::User,
            ChatMessageRole::Assistant,
            ChatMessageRole::Toolcall,
            ChatMessageRole::Toolcall,
            ChatMessageRole::ToolResult,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
    assert_eq!(
        first_turn.turn.snapshot.non_system_messages()[2].tool_calls[0].id,
        "call_demo_batch_1"
    );
    assert_eq!(
        first_turn.turn.snapshot.non_system_messages()[3].tool_calls[0].id,
        "call_demo_batch_2"
    );
    assert_eq!(final_turn.turn.reply, "batch done");
}

#[tokio::test]
async fn run_v1_executes_tool_calls_and_persists_tool_events_in_snapshot() {
    // 测试场景: assistant/toolcall/tool_result 都按正式 message 进入 Thread，并在 commit 后立即外发。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(EchoTool))
        .await
        .expect("echo tool should register");
    let provider = ScriptedLLMProvider::new(vec![
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "我先查一下",
                Utc::now(),
            )),
            vec![scripted_tool_call(
                "call_demo_1",
                "demo__echo",
                json!({"query": "demo"}),
            )],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "done",
                Utc::now(),
            )),
            Vec::new(),
        ),
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_loop_tool");
    let incoming = build_incoming("run tool", "thread_loop_tool");

    let (output, events) =
        run_locked_thread_with_recorded_events(&loop_runner, &incoming, thread_context).await;

    let final_turn = last_turn(&output);
    assert_eq!(
        final_turn
            .turn
            .snapshot
            .non_system_messages()
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            ChatMessageRole::User,
            ChatMessageRole::Assistant,
            ChatMessageRole::Toolcall,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
    assert_eq!(
        final_turn.turn.snapshot.non_system_messages()[2].tool_calls[0].id,
        "call_demo_1"
    );
    assert_eq!(final_turn.turn.snapshot.load_tool_events().len(), 1);
    assert_eq!(
        final_turn.turn.snapshot.load_tool_events()[0]
            .tool_name
            .as_deref(),
        Some("demo__echo")
    );
    assert_eq!(
        collected_event_kinds(&events),
        vec![
            AgentLoopEventKind::TextOutput,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::TextOutput,
        ]
    );
}

#[tokio::test]
async fn run_v1_preserves_reasoning_and_multi_tool_order_across_iterations() {
    // 测试场景: Responses 风格的 reasoning -> tool_call -> tool_result -> final assistant 链路必须保序写入 Thread。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(WeatherTool))
        .await
        .expect("weather tool should register");
    runtime
        .tools()
        .register(Arc::new(RouteTool))
        .await
        .expect("route tool should register");
    runtime
        .tools()
        .register(Arc::new(HotelTool))
        .await
        .expect("hotel tool should register");
    let provider = ScriptedLLMProvider::new(vec![
        LLMResponse {
            items: vec![
                ChatMessage::new(ChatMessageRole::Reasoning, "先看杭州天气", Utc::now())
                    .with_provider_item_id("rsn_weather"),
                ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now())
                    .with_provider_item_id("fc_weather")
                    .with_tool_calls(vec![scripted_tool_call(
                        "call_weather",
                        "demo__weather",
                        json!({"city": "杭州"}),
                    )]),
            ],
        },
        LLMResponse {
            items: vec![
                ChatMessage::new(
                    ChatMessageRole::Reasoning,
                    "天气适合出行，再查路线和酒店",
                    Utc::now(),
                )
                .with_provider_item_id("rsn_trip"),
                ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now())
                    .with_provider_item_id("fc_route")
                    .with_tool_calls(vec![scripted_tool_call(
                        "call_route",
                        "demo__route",
                        json!({"from": "上海", "to": "杭州"}),
                    )]),
                ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now())
                    .with_provider_item_id("fc_hotel")
                    .with_tool_calls(vec![scripted_tool_call(
                        "call_hotel",
                        "demo__hotel",
                        json!({"city": "杭州", "nights": 2}),
                    )]),
            ],
        },
        LLMResponse {
            items: vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                "已为你整理杭州三日行程",
                Utc::now(),
            )],
        },
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_responses_reasoning");
    let incoming = build_incoming("帮我规划杭州行程", "thread_responses_reasoning");

    let (output, events) =
        run_locked_thread_with_recorded_events(&loop_runner, &incoming, thread_context).await;

    let snapshot_roles = last_turn(&output)
        .turn
        .snapshot
        .non_system_messages()
        .iter()
        .map(|message| message.role.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        snapshot_roles,
        vec![
            ChatMessageRole::User,
            ChatMessageRole::Reasoning,
            ChatMessageRole::Toolcall,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Reasoning,
            ChatMessageRole::Toolcall,
            ChatMessageRole::Toolcall,
            ChatMessageRole::ToolResult,
            ChatMessageRole::ToolResult,
            ChatMessageRole::Assistant,
        ]
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[1]
            .provider_item_id
            .as_deref(),
        Some("rsn_weather")
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[4]
            .provider_item_id
            .as_deref(),
        Some("rsn_trip")
    );
    assert_eq!(
        collected_event_kinds(&events),
        vec![
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolCall,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::ToolResult,
            AgentLoopEventKind::TextOutput,
        ]
    );
    assert_eq!(last_turn(&output).turn.reply, "已为你整理杭州三日行程");
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
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "准备调用长工具",
                Utc::now(),
            )),
            vec![scripted_tool_call(
                "call_demo_long_1",
                "demo__long_echo",
                json!({"query": "demo"}),
            )],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "long done",
                Utc::now(),
            )),
            Vec::new(),
        ),
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        runtime,
        LLMConfig::default(),
        AgentCompactConfig::default(),
    );
    let thread_context = build_thread("thread_long_tool");
    let incoming = build_incoming("run long tool", "thread_long_tool");

    let (output, events) =
        run_locked_thread_with_recorded_events(&loop_runner, &incoming, thread_context).await;

    let tool_result_event = events
        .iter()
        .find(|event| event.kind == AgentLoopEventKind::ToolResult)
        .expect("tool result event should exist");
    assert!(tool_result_event.content.contains("...(truncated"));
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[3].content,
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
        scripted_llm_response(
            None,
            vec![scripted_tool_call("call_compact_1", "compact", json!({}))],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "after compact",
                Utc::now(),
            )),
            Vec::new(),
        ),
    ]);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(provider),
        AgentRuntime::new(),
        LLMConfig::default(),
        compact_config,
    );
    let mut thread_context = build_thread("thread_loop_compact");
    let now = Utc::now();
    thread_context.commit_test_turn(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::User, "old request", now),
            ChatMessage::new(ChatMessageRole::Assistant, "old reply", now),
        ],
        now,
        now,
    );
    let incoming = build_incoming("continue", "thread_loop_compact");

    let mut probe = RecordingUTProbe::default();
    let output = loop_runner
        .run_v1_with_ut_probe(
            build_event_sender(&incoming, &thread_context),
            &incoming,
            thread_context,
            Some(&mut probe),
        )
        .await
        .expect("loop should succeed");

    let compact_result_snapshot = probe
        .compacts
        .last()
        .expect("compact snapshot should be recorded")
        .request_messages
        .iter()
        .filter(|message| message.role != ChatMessageRole::System)
        .cloned()
        .collect::<Vec<_>>();
    let final_turn = last_turn(&output);
    assert_eq!(output.reply, "after compact");
    assert_eq!(probe.compacts.len(), 1);
    assert_eq!(
        final_turn
            .turn
            .snapshot
            .non_system_messages()
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
    assert_eq!(compact_result_snapshot.len(), 2);
    assert_eq!(
        compact_result_snapshot[0].content,
        "这是压缩后的上下文，请基于这些信息继续当前任务：\n任务已压缩"
    );
    assert_eq!(compact_result_snapshot[1].content, "继续");
    assert_eq!(final_turn.turn.snapshot.non_system_messages().len(), 4);
    assert_eq!(
        final_turn.turn.snapshot.non_system_messages()[1].content,
        "继续"
    );
    assert_eq!(
        final_turn.turn.snapshot.non_system_messages()[3].content,
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
        scripted_llm_response(
            None,
            vec![scripted_tool_call(
                "call_auto_compact_1",
                "compact",
                json!({}),
            )],
        ),
        scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "after auto compact",
                Utc::now(),
            )),
            Vec::new(),
        ),
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
    thread_context.commit_test_turn(None, history, now, now);
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
    assert_eq!(probe.compacts[0].request_messages.len(), 7);
    assert_eq!(last_turn(&output).turn.snapshot.system_messages().len(), 5);
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
            .non_system_messages()
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
        last_turn(&output).turn.snapshot.non_system_messages()[0].content,
        "这是压缩后的上下文，请基于这些信息继续当前任务：\n自动压缩摘要"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[1].content,
        "继续"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[2].content,
        "compact completed: compacted 11 messages from current chat history"
    );
    assert_eq!(
        last_turn(&output).turn.snapshot.non_system_messages()[3].content,
        "after auto compact"
    );
}
