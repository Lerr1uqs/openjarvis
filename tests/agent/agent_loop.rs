use anyhow::Result;
use async_trait::async_trait;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEventKind, AgentRuntime,
        HookEvent, HookEventKind, HookHandler, HookRegistry, InfoContext,
    },
    context::{ChatMessage, ChatMessageRole, MessageContext},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::ReplyTarget,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};

use super::tool::mcp::demo_stdio_config;

struct RecordingHook {
    kinds: Arc<Mutex<Vec<HookEventKind>>>,
    payloads: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl HookHandler for RecordingHook {
    fn name(&self) -> &'static str {
        "loop_recording_hook"
    }

    async fn handle(&self, event: &HookEvent) -> Result<()> {
        self.kinds.lock().await.push(event.kind.clone());
        self.payloads.lock().await.push(event.payload.clone());
        Ok(())
    }
}

#[tokio::test]
async fn agent_loop_emits_hooks_and_returns_reply() {
    let hooks = Arc::new(HookRegistry::new());
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    hooks
        .register(Arc::new(RecordingHook {
            kinds: Arc::clone(&kinds),
            payloads: Arc::clone(&payloads),
        }))
        .await;

    let runtime = AgentRuntime::with_parts(
        Arc::clone(&hooks),
        Arc::new(openjarvis::agent::ToolRegistry::new()),
    );
    let loop_runner = AgentLoop::new(Arc::new(MockLLMProvider::new("loop-reply")), runtime);
    let (input, outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "hello"))
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 1).await;

    let emitted_kinds = kinds.lock().await.clone();
    let emitted_payloads = payloads.lock().await.clone();

    assert_eq!(output.reply, "loop-reply");
    assert_eq!(output.metadata["tool_count"], 4);
    assert_eq!(output.events.len(), 1);
    assert_eq!(output.turn_messages.len(), 1);
    assert_eq!(output.turn_messages[0].content, "loop-reply");
    assert_eq!(outgoing[0].content, "loop-reply");
    assert_eq!(format!("{:?}", outgoing[0].kind), "TextOutput");
    assert_eq!(
        emitted_kinds,
        vec![HookEventKind::UserPromptSubmit, HookEventKind::Notification]
    );
    assert_eq!(emitted_payloads[0]["channel"], "feishu");
    assert_eq!(output.metadata["hook_handler_count"], 1);
}

struct SequenceProvider {
    responses: Arc<Mutex<Vec<LLMResponse>>>,
}

#[async_trait]
impl LLMProvider for SequenceProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        let mut responses = self.responses.lock().await;
        Ok(responses.remove(0))
    }
}

#[tokio::test]
async fn agent_loop_runs_single_tool_round_and_returns_final_answer() {
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("read", serde_json::json!({ "path": "Cargo.toml" })),
                text_response("读取完成"),
            ])),
        }),
        runtime,
    );
    let (input, outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "请读取 Cargo.toml"))
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 3).await;

    assert_eq!(output.reply, "读取完成");
    assert_eq!(output.metadata["used_tool_name"], "read");
    assert_eq!(output.events.len(), 3);
    assert_eq!(output.turn_messages.len(), 3);
    assert_eq!(output.turn_messages[0].tool_calls[0].id, "call_test_1");
    assert_eq!(
        output.turn_messages[1].tool_call_id.as_deref(),
        Some("call_test_1")
    );
    assert_eq!(output.turn_messages[2].content, "读取完成");
    assert_eq!(output.events[0].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::TextOutput);
    assert!(outgoing[0].content.contains("[openjarvis][tool_call]"));
    assert!(outgoing[1].content.contains("[openjarvis][tool_result]"));
    assert_eq!(outgoing[2].content, "读取完成");
}

#[tokio::test]
async fn agent_loop_can_be_driven_by_mock_provider_to_verify_tool_hooks() {
    let hooks = Arc::new(HookRegistry::new());
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    hooks
        .register(Arc::new(RecordingHook {
            kinds: Arc::clone(&kinds),
            payloads: Arc::clone(&payloads),
        }))
        .await;

    let runtime = AgentRuntime::with_parts(
        Arc::clone(&hooks),
        Arc::new(openjarvis::agent::ToolRegistry::new()),
    );
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("read", serde_json::json!({ "path": "Cargo.toml" })),
                text_response("读取完成"),
            ])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "请读取 Cargo.toml"))
        .await
        .expect("loop should succeed");

    let emitted_kinds = kinds.lock().await.clone();
    let emitted_payloads = payloads.lock().await.clone();

    assert_eq!(output.reply, "读取完成");
    assert_eq!(
        emitted_kinds,
        vec![
            HookEventKind::UserPromptSubmit,
            HookEventKind::PreToolUse,
            HookEventKind::PostToolUse,
            HookEventKind::Notification,
        ]
    );
    assert_eq!(emitted_payloads[1]["tool"], "read");
    assert_eq!(emitted_payloads[1]["tool_call_id"], "call_test_1");
    assert_eq!(emitted_payloads[2]["tool"], "read");
    assert_eq!(emitted_payloads[2]["result"]["path"], "Cargo.toml");
    assert!(emitted_payloads[2]["result"]["returned_line_count"].is_number());
}

#[tokio::test]
async fn agent_loop_emits_post_tool_use_failure_when_mock_provider_requests_unknown_tool() {
    let hooks = Arc::new(HookRegistry::new());
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    hooks
        .register(Arc::new(RecordingHook {
            kinds: Arc::clone(&kinds),
            payloads: Arc::clone(&payloads),
        }))
        .await;

    let runtime = AgentRuntime::with_parts(
        Arc::clone(&hooks),
        Arc::new(openjarvis::agent::ToolRegistry::new()),
    );
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("missing_tool", serde_json::json!({})),
                text_response("失败已处理"),
            ])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "执行一个不存在的工具"))
        .await
        .expect("loop should succeed even when one tool call fails");

    let emitted_kinds = kinds.lock().await.clone();
    let emitted_payloads = payloads.lock().await.clone();

    assert_eq!(output.reply, "失败已处理");
    assert_eq!(
        emitted_kinds,
        vec![
            HookEventKind::UserPromptSubmit,
            HookEventKind::PreToolUse,
            HookEventKind::PostToolUseFailure,
            HookEventKind::Notification,
        ]
    );
    assert_eq!(emitted_payloads[1]["tool"], "missing_tool");
    assert_eq!(emitted_payloads[2]["tool"], "missing_tool");
    assert!(
        emitted_payloads[2]["error"]
            .as_str()
            .expect("failure payload should contain error text")
            .contains("missing_tool")
    );
}

#[tokio::test]
async fn agent_loop_accepts_protocol_prefix_with_colon() {
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("read", serde_json::json!({ "path": "Cargo.toml" })),
                text_response("读取完成"),
            ])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "请读取 Cargo.toml"))
        .await
        .expect("loop should succeed");

    assert_eq!(output.reply, "读取完成");
    assert_eq!(output.metadata["used_tool_name"], "read");
}

#[tokio::test]
async fn agent_loop_emits_response_before_tool_call_when_both_exist() {
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                text_and_tool_response(
                    "我先看看文件内容",
                    "read",
                    serde_json::json!({ "path": "Cargo.toml" }),
                ),
                text_response("读取完成"),
            ])),
        }),
        runtime,
    );
    let (input, outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "请读取 Cargo.toml"))
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 4).await;

    assert_eq!(output.events.len(), 4);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(output.events[0].content, "我先看看文件内容");
    assert_eq!(output.events[1].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[3].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(output.turn_messages[0].content, "我先看看文件内容");
    assert_eq!(output.turn_messages[0].tool_calls[0].id, "call_test_1");
    assert_eq!(outgoing[0].content, "我先看看文件内容");
    assert_eq!(outgoing[3].content, "读取完成");
}

#[tokio::test]
async fn agent_loop_executes_all_tool_calls_in_one_response() {
    let runtime = AgentRuntime::new();
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                multi_tool_response(vec![
                    ("read", serde_json::json!({ "path": "Cargo.toml" })),
                    ("read", serde_json::json!({ "path": "README.md" })),
                ]),
                text_response("全部读取完成"),
            ])),
        }),
        runtime,
    );
    let (input, outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "读取两个文件"))
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 5).await;

    assert_eq!(output.reply, "全部读取完成");
    assert_eq!(output.events.len(), 5);
    assert_eq!(
        output
            .events
            .iter()
            .filter(|event| event.kind == AgentLoopEventKind::ToolCall)
            .count(),
        2
    );
    assert_eq!(
        output
            .events
            .iter()
            .filter(|event| event.kind == AgentLoopEventKind::ToolResult)
            .count(),
        2
    );
    assert_eq!(
        outgoing.last().map(|message| message.content.as_str()),
        Some("全部读取完成")
    );
}

#[tokio::test]
async fn agent_loop_executes_namespaced_mcp_tool_calls() {
    let config = demo_stdio_config(true);
    let runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("runtime should build with demo stdio MCP");
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response(
                    "mcp__demo_stdio__echo",
                    serde_json::json!({ "text": "来自 agent loop" }),
                ),
                text_response("MCP 调用完成"),
            ])),
        }),
        runtime,
    );
    let (input, outgoing_rx) = build_input();

    let output = loop_runner
        .run(input, &build_context("system", "调用 demo MCP"))
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 3).await;

    assert_eq!(output.reply, "MCP 调用完成");
    assert_eq!(output.metadata["used_tool_name"], "mcp__demo_stdio__echo");
    assert_eq!(output.metadata["tool_count"], 7);
    assert_eq!(output.metadata["mcp_server_count"], 1);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::TextOutput);
    assert!(outgoing[0].content.contains("mcp__demo_stdio__echo"));
    assert!(outgoing[1].content.contains("[demo:stdio] 来自 agent loop"));
    assert_eq!(outgoing[2].content, "MCP 调用完成");
    assert_eq!(
        output.turn_messages[1].content,
        "[demo:stdio] 来自 agent loop"
    );
}

fn build_context(system_prompt: &str, user_message: &str) -> MessageContext {
    // 作用: 为 agent loop 测试构造一个最小上下文，模拟 worker 从 session 中拼好的 MessageContext。
    // 参数: system_prompt 为系统提示词，user_message 为用户输入文本。
    let mut context = MessageContext::with_system_prompt(system_prompt);
    context.chat.push(openjarvis::context::ChatMessage::new(
        openjarvis::context::ChatMessageRole::User,
        user_message,
        chrono::Utc::now(),
    ));
    context
}

fn build_input() -> (InfoContext, mpsc::Receiver<AgentDispatchEvent>) {
    let (tx, rx) = mpsc::channel(32);
    (
        InfoContext {
            channel: "feishu".to_string(),
            user_id: "ou_xxx".to_string(),
            thread_id: "thread_1".to_string(),
            event_tx: AgentEventSender::new(
                tx,
                "feishu",
                Some("thread_1".to_string()),
                Some("msg_1".to_string()),
                ReplyTarget {
                    receive_id: "oc_xxx".to_string(),
                    receive_id_type: "chat_id".to_string(),
                },
                "session_1",
                "feishu",
                "ou_xxx",
                "thread_1",
                "thread_1",
            ),
        },
        rx,
    )
}

async fn collect_outgoing(
    mut outgoing_rx: mpsc::Receiver<AgentDispatchEvent>,
    expected_count: usize,
) -> Vec<AgentDispatchEvent> {
    timeout(Duration::from_millis(500), async move {
        let mut messages = Vec::new();
        while messages.len() < expected_count {
            let message = outgoing_rx
                .recv()
                .await
                .expect("outgoing message channel should stay open");
            messages.push(message);
        }
        messages
    })
    .await
    .expect("outgoing messages should be emitted")
}

fn text_response(content: &str) -> LLMResponse {
    // 作用: 为 agent loop 单测构造仅含 assistant 文本的模型返回。
    // 参数: content 为 assistant 文本内容。
    LLMResponse {
        message: Some(ChatMessage::new(
            ChatMessageRole::Assistant,
            content,
            chrono::Utc::now(),
        )),
        tool_calls: Vec::new(),
    }
}

fn tool_only_response(name: &str, arguments: serde_json::Value) -> LLMResponse {
    // 作用: 为 agent loop 单测构造仅包含原生 tool_call 的模型返回。
    // 参数: name 为工具名，arguments 为工具参数。
    LLMResponse {
        message: None,
        tool_calls: vec![LLMToolCall {
            id: "call_test_1".to_string(),
            name: name.to_string(),
            arguments,
        }],
    }
}

fn text_and_tool_response(content: &str, name: &str, arguments: serde_json::Value) -> LLMResponse {
    // 作用: 为 agent loop 单测构造同时包含 assistant 文本和 tool_call 的模型返回。
    // 参数: content 为 assistant 文本，name 为工具名，arguments 为工具参数。
    LLMResponse {
        message: Some(ChatMessage::new(
            ChatMessageRole::Assistant,
            content,
            chrono::Utc::now(),
        )),
        tool_calls: vec![LLMToolCall {
            id: "call_test_1".to_string(),
            name: name.to_string(),
            arguments,
        }],
    }
}

fn multi_tool_response(calls: Vec<(&str, serde_json::Value)>) -> LLMResponse {
    LLMResponse {
        message: None,
        tool_calls: calls
            .into_iter()
            .enumerate()
            .map(|(index, (name, arguments))| LLMToolCall {
                id: format!("call_test_{}", index + 1),
                name: name.to_string(),
                arguments,
            })
            .collect(),
    }
}
