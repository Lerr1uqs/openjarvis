#![allow(deprecated)]

use anyhow::Result;
use async_trait::async_trait;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEventKind, AgentRuntime,
        HookEvent, HookEventKind, HookHandler, HookRegistry, InfoContext, ToolCallRequest,
        ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry, ToolsetCatalogEntry,
        empty_tool_input_schema,
    },
    compact::CompactScopeKey,
    config::{AgentCompactConfig, AppConfig, LLMConfig},
    context::{ChatMessage, ChatMessageRole, MessageContext},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    thread::{ConversationThread, ThreadContext, ThreadContextLocator},
};
use serde_json::Value;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};

use super::tool::mcp::demo_stdio_config;
use super::tool::skill::SkillFixture;

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
        Arc::new(openjarvis::agent::ToolRegistry::with_skill_roots(Vec::new())),
    );
    let loop_runner = AgentLoop::new(Arc::new(MockLLMProvider::new("loop-reply")), runtime);
    let (input, outgoing_rx) = build_input();

    let output = run_simple_turn(&loop_runner, input, "system", "hello")
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 1).await;

    let emitted_kinds = kinds.lock().await.clone();
    let emitted_payloads = payloads.lock().await.clone();

    assert_eq!(output.reply, "loop-reply");
    assert_eq!(output.metadata["tool_count"], 6);
    assert_eq!(output.events.len(), 1);
    assert_eq!(output.commit_messages.len(), 1);
    assert_eq!(output.commit_messages[0].content, "loop-reply");
    assert_eq!(outgoing[0].content, "loop-reply");
    assert_eq!(format!("{:?}", outgoing[0].kind), "TextOutput");
    assert_eq!(
        emitted_kinds,
        vec![HookEventKind::UserPromptSubmit, HookEventKind::Notification]
    );
    assert_eq!(emitted_payloads[0]["channel"], "feishu");
    assert_eq!(output.metadata["hook_handler_count"], 1);
}

#[tokio::test]
async fn agent_loop_initializes_empty_thread_before_appending_current_user_message() {
    // 测试场景: run_v1 对空 thread 应先执行 thread_is_initialized 判空与 thread_init，
    // 再把当前用户消息放入 live chat，避免用户输入污染初始化判断。
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config_and_system_prompt(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("初始化完成")])),
        }),
        runtime_without_skills(),
        LLMConfig::default(),
        AgentCompactConfig::default(),
        Some("thread system prompt".to_string()),
    );
    let (input, _outgoing_rx) = build_input();
    let thread_context =
        ThreadContext::new(thread_context_locator_for_input(&input), chrono::Utc::now());
    let incoming = build_incoming_for_input(&input, "当前问题");

    let output = loop_runner
        .run_v1(input.event_tx, &incoming, thread_context)
        .await
        .expect("loop should initialize empty thread before first request");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 1);
    assert_eq!(
        captured_requests[0].messages[0].content,
        "thread system prompt"
    );
    assert_eq!(
        captured_requests[0]
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("当前问题")
    );
    assert_eq!(
        output.thread_context.request_context_system_messages()[0].content,
        "thread system prompt"
    );
    assert_eq!(output.reply, "初始化完成");
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

struct RecordingSequenceProvider {
    requests: Arc<Mutex<Vec<LLMRequest>>>,
    responses: Arc<Mutex<Vec<LLMResponse>>>,
}

#[async_trait]
impl LLMProvider for RecordingSequenceProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        self.requests.lock().await.push(request);
        let mut responses = self.responses.lock().await;
        Ok(responses.remove(0))
    }
}

struct DemoLoopTool;

#[async_trait]
impl ToolHandler for DemoLoopTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo from the demo loop toolset".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "demo tool ok".to_string(),
            metadata: serde_json::json!({ "toolset": "demo" }),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn agent_loop_runs_single_tool_round_and_returns_final_answer() {
    let runtime = runtime_without_skills();
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

    let output = run_simple_turn(&loop_runner, input, "system", "请读取 Cargo.toml")
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 3).await;

    assert_eq!(output.reply, "读取完成");
    assert_eq!(output.metadata["used_tool_name"], "read");
    assert_eq!(output.events.len(), 3);
    assert_eq!(output.commit_messages.len(), 3);
    assert_eq!(output.commit_messages[0].tool_calls[0].id, "call_test_1");
    assert_eq!(
        output.commit_messages[1].tool_call_id.as_deref(),
        Some("call_test_1")
    );
    assert_eq!(output.commit_messages[2].content, "读取完成");
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
        Arc::new(openjarvis::agent::ToolRegistry::with_skill_roots(Vec::new())),
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

    let output = run_simple_turn(&loop_runner, input, "system", "请读取 Cargo.toml")
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
        Arc::new(openjarvis::agent::ToolRegistry::with_skill_roots(Vec::new())),
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

    let output = run_simple_turn(&loop_runner, input, "system", "执行一个不存在的工具")
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
    let runtime = runtime_without_skills();
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

    let output = run_simple_turn(&loop_runner, input, "system", "请读取 Cargo.toml")
        .await
        .expect("loop should succeed");

    assert_eq!(output.reply, "读取完成");
    assert_eq!(output.metadata["used_tool_name"], "read");
}

#[tokio::test]
async fn agent_loop_emits_response_before_tool_call_when_both_exist() {
    let runtime = runtime_without_skills();
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

    let output = run_simple_turn(&loop_runner, input, "system", "请读取 Cargo.toml")
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 4).await;

    assert_eq!(output.events.len(), 4);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(output.events[0].content, "我先看看文件内容");
    assert_eq!(output.events[1].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[3].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(output.commit_messages[0].content, "我先看看文件内容");
    assert_eq!(output.commit_messages[0].tool_calls[0].id, "call_test_1");
    assert_eq!(outgoing[0].content, "我先看看文件内容");
    assert_eq!(outgoing[3].content, "读取完成");
}

#[tokio::test]
async fn agent_loop_executes_all_tool_calls_in_one_response() {
    let runtime = runtime_without_skills();
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

    let output = run_simple_turn(&loop_runner, input, "system", "读取两个文件")
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
    let runtime = AgentRuntime::from_config_with_skill_roots(config.agent_config(), Vec::new())
        .await
        .expect("runtime should build with demo stdio MCP");
    let loop_runner = AgentLoop::new(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("load_toolset", serde_json::json!({ "name": "demo_stdio" })),
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

    let output = run_simple_turn(&loop_runner, input, "system", "调用 demo MCP")
        .await
        .expect("loop should succeed");
    let outgoing = collect_outgoing(outgoing_rx, 5).await;

    assert_eq!(output.reply, "MCP 调用完成");
    assert_eq!(output.metadata["used_tool_name"], "load_toolset");
    assert_eq!(
        output.metadata["used_tool_names"],
        serde_json::json!(["load_toolset", "mcp__demo_stdio__echo"])
    );
    assert_eq!(output.metadata["tool_count"], 9);
    assert_eq!(output.metadata["mcp_server_count"], 1);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[3].kind, AgentLoopEventKind::ToolResult);
    assert_eq!(output.events[4].kind, AgentLoopEventKind::TextOutput);
    assert!(outgoing[0].content.contains("load_toolset"));
    assert!(outgoing[2].content.contains("mcp__demo_stdio__echo"));
    assert!(outgoing[3].content.contains("[demo:stdio] 来自 agent loop"));
    assert_eq!(outgoing[4].content, "MCP 调用完成");
    assert_eq!(output.loaded_toolsets, vec!["demo_stdio".to_string()]);
    assert_eq!(output.tool_events.len(), 2);
    assert_eq!(
        output.commit_messages[3].content,
        "[demo:stdio] 来自 agent loop"
    );
}

#[tokio::test]
async fn agent_loop_refreshes_tools_after_load_and_hides_them_after_unload_in_same_turn() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo toolset for loop refresh"),
            vec![Arc::new(DemoLoopTool)],
        )
        .await
        .expect("demo toolset should register");
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), registry);
    let loop_runner = AgentLoop::new(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("load_toolset", serde_json::json!({ "name": "demo" })),
                tool_only_response("demo__echo", serde_json::json!({})),
                tool_only_response("unload_toolset", serde_json::json!({ "name": "demo" })),
                text_response("done"),
            ])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = run_simple_turn(
        &loop_runner,
        input,
        "system",
        "load demo toolset, use it, then unload it",
    )
    .await
    .expect("loop should succeed");

    let captured_requests = requests.lock().await;
    let first_tools = captured_requests[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let second_tools = captured_requests[1]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let fourth_tools = captured_requests[3]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let first_messages = captured_requests[0]
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(output.reply, "done");
    assert!(first_tools.contains(&"load_toolset"));
    assert!(!first_tools.contains(&"demo__echo"));
    assert!(second_tools.contains(&"demo__echo"));
    assert!(!fourth_tools.contains(&"demo__echo"));
    assert!(
        first_messages
            .iter()
            .any(|content| content.contains("- demo: Demo toolset for loop refresh")),
        "toolset catalog prompt was not injected: {first_messages:?}"
    );
    assert!(output.loaded_toolsets.is_empty());
    assert_eq!(output.tool_events.len(), 3);
    assert_eq!(output.tool_events[0].toolset_name.as_deref(), Some("demo"));
}

#[tokio::test]
async fn agent_loop_run_with_thread_context_keeps_toolsets_isolated_per_thread() {
    // 测试场景: AgentLoop 改为直接消费 ThreadContext 后，不同线程的 loaded toolset 不能互相串线。
    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo isolated toolset"),
            vec![Arc::new(DemoLoopTool)],
        )
        .await
        .expect("demo toolset should register");
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), registry);
    let loop_runner = AgentLoop::new(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                text_response("thread-a-done"),
                text_response("thread-b-done"),
            ])),
        }),
        runtime,
    );
    let (input_a, _outgoing_a) = build_input_for("thread_a_internal", "thread_a");
    let (input_b, _outgoing_b) = build_input_for("thread_b_internal", "thread_b");
    let mut thread_a = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_a", "thread_a_internal"),
        chrono::Utc::now(),
    );
    let thread_b = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_b", "thread_b_internal"),
        chrono::Utc::now(),
    );
    assert!(thread_a.load_toolset("demo"));

    let output_a = loop_runner
        .run_with_thread_context(input_a, &build_context("system", "处理线程A"), thread_a)
        .await
        .expect("thread A loop should succeed");
    let output_b = loop_runner
        .run_with_thread_context(input_b, &build_context("system", "处理线程B"), thread_b)
        .await
        .expect("thread B loop should succeed");

    let captured_requests = requests.lock().await;
    let first_tools = captured_requests[0]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let second_tools = captured_requests[1]
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();

    assert!(first_tools.contains(&"demo__echo"));
    assert!(!second_tools.contains(&"demo__echo"));
    assert_eq!(output_a.loaded_toolsets, vec!["demo".to_string()]);
    assert!(output_b.loaded_toolsets.is_empty());
}

#[tokio::test]
async fn agent_loop_does_not_inject_skill_prompt_or_tool_when_no_local_skills_exist() {
    let fixture = SkillFixture::new("openjarvis-agent-loop-no-skills");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = AgentRuntime::with_parts(
        Arc::new(HookRegistry::new()),
        Arc::new(ToolRegistry::with_skill_roots(vec![
            fixture.skills_root().to_path_buf(),
        ])),
    );
    let loop_runner = AgentLoop::new(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("无技能回复")])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = run_simple_turn(&loop_runner, input, "system", "普通问题")
        .await
        .expect("loop should succeed");

    let captured_requests = requests.lock().await;
    let first_request = &captured_requests[0];
    let tool_names = first_request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let serialized_messages = first_request
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(output.reply, "无技能回复");
    assert!(!tool_names.contains(&"load_skill"));
    assert!(
        !serialized_messages
            .iter()
            .any(|content| content.contains("You have access to local skills")),
        "unexpected skill prompt leaked into request: {serialized_messages:?}"
    );
}

#[tokio::test]
async fn agent_loop_injects_skill_prompt_and_progressively_loads_skill_content() {
    let fixture = SkillFixture::new("openjarvis-agent-loop-with-skills");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: help with demo workflows
---
Read `guide.md` before replying.
"#,
    );
    fixture.write_skill_file("demo_skill", "guide.md", "guide content");

    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = AgentRuntime::with_parts(
        Arc::new(HookRegistry::new()),
        Arc::new(ToolRegistry::with_skill_roots(vec![
            fixture.skills_root().to_path_buf(),
        ])),
    );
    let loop_runner = AgentLoop::new(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("load_skill", serde_json::json!({ "name": "demo_skill" })),
                text_response("技能加载完成"),
            ])),
        }),
        runtime,
    );
    let (input, _outgoing_rx) = build_input();

    let output = run_simple_turn(&loop_runner, input, "system", "请处理 demo 工作流")
        .await
        .expect("loop should succeed");

    let captured_requests = requests.lock().await;
    let first_request = &captured_requests[0];
    let second_request = &captured_requests[1];
    let first_tool_names = first_request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let first_messages = first_request
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    let second_messages = second_request
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(output.reply, "技能加载完成");
    assert_eq!(output.metadata["used_tool_name"], "load_skill");
    assert!(first_tool_names.contains(&"load_skill"));
    assert!(
        first_messages.iter().any(|content| content
            .contains("Available local skills:\n- demo_skill: help with demo workflows")),
        "skill catalog prompt was not injected: {first_messages:?}"
    );
    assert!(
        second_messages
            .iter()
            .any(|content| content.contains("Loaded local skill `demo_skill`.")),
        "loaded skill prompt was not appended to the next request: {second_messages:?}"
    );
    assert!(
        second_messages
            .iter()
            .any(|content| content.contains("guide content")),
        "referenced skill file content was not propagated: {second_messages:?}"
    );
}

#[tokio::test]
async fn agent_loop_rebuilds_fixed_feature_slots_before_persisted_history() {
    // 测试场景: loop 发起请求前应先 rebuild `features_system_prompt`，
    // 再通过 ThreadContext.messages() 导出 persisted snapshot -> features_system_prompt
    // -> live system -> live memory -> history -> live chat。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    auto_compact: true
    runtime_threshold_ratio: 1.0
    tool_visible_threshold_ratio: 0.95
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 20000
  tokenizer: "chars_div4"
"#,
    )
    .expect("auto compact config should parse");
    let fixture = SkillFixture::new("openjarvis-agent-loop-feature-order");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: help with ordered feature prompt tests
---
Read `guide.md` before replying.
"#,
    );
    fixture.write_skill_file("demo_skill", "guide.md", "guide content");

    let requests = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_skill_roots(vec![
        fixture.skills_root().to_path_buf(),
    ]));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo toolset for feature order"),
            vec![Arc::new(DemoLoopTool)],
        )
        .await
        .expect("demo toolset should register");
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), registry);
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("按顺序完成")])),
        }),
        runtime,
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_1", "thread_1"),
        now,
    );
    assert!(thread_context.ensure_system_prompt_snapshot("system", now));
    assert!(thread_context.load_toolset("demo"));
    thread_context.enable_auto_compact();
    thread_context.store_turn(
        None,
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "persisted history",
            now,
        )],
        now,
        now,
    );
    let context = build_context("system", "当前问题");

    let output = loop_runner
        .run_with_thread_context(input, &context, thread_context)
        .await
        .expect("loop should rebuild fixed feature slots before request");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 1);
    let request_messages = captured_requests[0]
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    let find_contains_index = |needle: &str| {
        request_messages
            .iter()
            .position(|content| content.contains(needle))
            .expect("expected request message should exist")
    };
    let find_exact_index = |needle: &str| {
        request_messages
            .iter()
            .position(|content| *content == needle)
            .expect("expected exact request message should exist")
    };

    let snapshot_index = find_exact_index("system");
    let tool_mode_index = find_contains_index("OpenJarvis tool-use mode");
    let toolset_index = find_contains_index("Demo toolset for feature order");
    let skill_index = find_contains_index("Available local skills");
    let auto_stable_index = find_contains_index("Auto-compact 已开启");
    let auto_dynamic_index = find_contains_index("<context capacity");
    let history_index = find_exact_index("persisted history");
    let user_index = find_exact_index("当前问题");

    assert!(snapshot_index < tool_mode_index);
    assert!(tool_mode_index < toolset_index);
    assert!(toolset_index < skill_index);
    assert!(skill_index < auto_stable_index);
    assert!(auto_stable_index < auto_dynamic_index);
    assert!(auto_dynamic_index < history_index);
    assert!(history_index < user_index);
    assert_eq!(output.reply, "按顺序完成");
}

#[tokio::test]
async fn agent_loop_runtime_compacts_persisted_history_before_final_llm_request() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    runtime_threshold_ratio: 0.25
    tool_visible_threshold_ratio: 0.9
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 1000
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                text_response(
                    "{\"compacted_assistant\":\"这是压缩后的上下文，任务目标是继续处理当前问题。\"}",
                ),
                text_response("压缩后继续"),
            ])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let active_thread = thread_with_history(vec![
        ChatMessage::new(
            ChatMessageRole::User,
            "这是一段很长的历史问题，需要被压缩。".repeat(40),
            chrono::Utc::now(),
        ),
        ChatMessage::new(
            ChatMessageRole::Assistant,
            "这是一段很长的历史回答，也需要被压缩。".repeat(40),
            chrono::Utc::now(),
        ),
    ]);

    let output =
        run_turn_with_active_thread(&loop_runner, input, "system", "新的问题", active_thread)
            .await
            .expect("loop should compact history before the final request");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 2);
    assert!(captured_requests[0].tools.is_empty());
    assert!(captured_requests[1].messages.iter().any(|message| {
        message
            .content
            .contains("这是压缩后的上下文，请基于这些信息继续当前任务")
    }));
    assert!(
        captured_requests[1]
            .messages
            .iter()
            .any(|message| message.content == "继续")
    );
    assert!(
        !captured_requests[1]
            .messages
            .iter()
            .any(|message| message.content == "新的问题")
    );
    assert_eq!(output.reply, "压缩后继续");
    assert_eq!(output.events[0].kind, AgentLoopEventKind::Compact);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::TextOutput);
    assert!(!output.persist_incoming_user);
    assert_eq!(output.active_thread.turns.len(), 1);
}

#[tokio::test]
async fn agent_loop_run_turn_ignores_legacy_request_memory_inputs_across_compact() {
    // 测试场景: loop 入口现在只消费一个当前 message；
    // 旧的 ContextMessage.memory 即使存在，也不能进入请求、线程持久状态或 compact 结果。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    runtime_threshold_ratio: 0.25
    tool_visible_threshold_ratio: 0.9
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 1000
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                text_response("{\"compacted_assistant\":\"这是压缩后的上下文，请继续当前任务。\"}"),
                text_response("带 memory 的压缩后继续"),
            ])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_1", "thread_1"),
        now,
    );
    assert!(thread_context.ensure_system_prompt_snapshot("system", now));
    thread_context.store_turn(
        None,
        vec![
            ChatMessage::new(
                ChatMessageRole::User,
                "这是一段很长的历史问题，需要被压缩。".repeat(40),
                now,
            ),
            ChatMessage::new(
                ChatMessageRole::Assistant,
                "这是一段很长的历史回答，也需要被压缩。".repeat(40),
                now,
            ),
        ],
        now,
        now,
    );

    let mut context = build_context("system", "新的问题");
    context.push_memory("transient memory only");
    let output = loop_runner
        .run_with_thread_context(input, &context, thread_context)
        .await
        .expect("loop should ignore legacy request memory inputs");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 2);
    assert!(
        !captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("transient memory only"))
    );
    assert!(
        !captured_requests[1]
            .messages
            .iter()
            .any(|message| message.content == "transient memory only")
    );
    assert!(
        !output
            .thread_context
            .request_context_system_messages()
            .iter()
            .any(|message| message.content == "transient memory only")
    );
    assert!(
        !output
            .thread_context
            .load_messages()
            .iter()
            .any(|message| message.content == "transient memory only")
    );
    assert_eq!(output.reply, "带 memory 的压缩后继续");
}

#[tokio::test]
async fn agent_loop_can_use_static_mock_compact_summary_without_extra_llm_call() {
    // 测试场景: 配置了 compact mock summary 后，历史压缩应直接走 StaticCompactProvider，
    // 不再额外消耗一次 LLM 请求去生成 compact 摘要。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    runtime_threshold_ratio: 0.25
    tool_visible_threshold_ratio: 0.9
    reserved_output_tokens: 16
    mock_compacted_assistant: "这是压缩后的上下文，使用 mock 保留任务状态。"
llm:
  provider: "mock"
  context_window_tokens: 1000
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact mock config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("mock 压缩后继续")])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let active_thread = thread_with_history(vec![
        ChatMessage::new(
            ChatMessageRole::User,
            "这是一段很长的历史问题，需要被压缩。".repeat(40),
            chrono::Utc::now(),
        ),
        ChatMessage::new(
            ChatMessageRole::Assistant,
            "这是一段很长的历史回答，也需要被压缩。".repeat(40),
            chrono::Utc::now(),
        ),
    ]);

    let output =
        run_turn_with_active_thread(&loop_runner, input, "system", "新的问题", active_thread)
            .await
            .expect("loop should compact history with static mock summary");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 1);
    assert!(captured_requests[0].messages.iter().any(|message| {
        message
            .content
            .contains("这是压缩后的上下文，使用 mock 保留任务状态。")
    }));
    assert_eq!(output.reply, "mock 压缩后继续");
    assert_eq!(output.events[0].kind, AgentLoopEventKind::Compact);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::TextOutput);
}

#[tokio::test]
async fn agent_loop_auto_compact_injects_status_prompt_before_budget_threshold() {
    // 测试场景: auto_compact 开启后，每次 generate 都应注入容量提示；
    // 即使预算远低于阈值，也要暴露 compact 工具，只是不升级为提前告警。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    auto_compact: true
    runtime_threshold_ratio: 1.0
    tool_visible_threshold_ratio: 0.95
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 20000
  tokenizer: "chars_div4"
"#,
    )
    .expect("auto compact config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("直接继续")])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();

    let output = run_simple_turn(&loop_runner, input, "system", "短消息")
        .await
        .expect("loop should expose compact tool before threshold");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 1);
    assert!(
        captured_requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "compact")
    );
    assert!(
        captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
    assert!(
        captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("`compact` 工具当前可用"))
    );
    assert!(
        !captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("超过 auto_compact 提前提醒阈值"))
    );
    assert_eq!(output.reply, "直接继续");
    assert_eq!(output.events.len(), 1);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::TextOutput);
}

#[tokio::test]
async fn agent_loop_runtime_override_can_enable_auto_compact_for_current_thread() {
    // 测试场景: 即使静态 compact 默认关闭，只要 ThreadContext 中持久化了线程级 override，
    // 后续轮次也应开启 auto-compact。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: false
    auto_compact: false
    runtime_threshold_ratio: 1.0
    tool_visible_threshold_ratio: 0.95
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 20000
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact override config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![text_response("override 生效")])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let mut active_thread = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_1", "thread_1"),
        chrono::Utc::now(),
    );
    active_thread.enable_auto_compact();

    let output = loop_runner
        .run_with_thread_context(input, &build_context("system", "普通问题"), active_thread)
        .await
        .expect("loop should honor thread-scoped auto-compact override");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 1);
    assert!(
        captured_requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "compact")
    );
    assert!(
        captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
    assert_eq!(output.reply, "override 生效");
}

#[tokio::test]
async fn agent_loop_runtime_override_can_execute_compact_tool_when_static_compact_disabled() {
    // 测试场景: 即使静态 compact 默认关闭，只要 ThreadContext 中的线程级 override 开启，
    // 模型调用 compact 也应真正成功。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: false
    auto_compact: false
    runtime_threshold_ratio: 1.0
    tool_visible_threshold_ratio: 0.1
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 3000
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact override config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("compact", serde_json::json!({})),
                text_response("{\"compacted_assistant\":\"这是 override 触发后的压缩上下文。\"}"),
                text_response("override compact 完成"),
            ])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let mut active_thread = ThreadContext::from_conversation_thread(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_1", "thread_1"),
        thread_with_history(vec![
            ChatMessage::new(
                ChatMessageRole::User,
                "需要被 override compact 的历史问题".repeat(15),
                chrono::Utc::now(),
            ),
            ChatMessage::new(
                ChatMessageRole::Assistant,
                "需要被 override compact 的历史回答".repeat(15),
                chrono::Utc::now(),
            ),
        ]),
    );
    active_thread.enable_auto_compact();

    let output = loop_runner
        .run_with_thread_context(input, &build_context("system", "请继续"), active_thread)
        .await
        .expect("loop should allow compact tool after runtime override");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 3);
    assert_eq!(output.events[0].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::Compact);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::TextOutput);
    assert_eq!(output.reply, "override compact 完成");
}

#[tokio::test]
async fn agent_loop_auto_compact_injects_budget_prompt_and_exposes_compact_tool() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    auto_compact: true
    runtime_threshold_ratio: 1.0
    tool_visible_threshold_ratio: 0.1
    reserved_output_tokens: 16
llm:
  provider: "mock"
  context_window_tokens: 3000
  tokenizer: "chars_div4"
"#,
    )
    .expect("auto compact config should parse");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loop_runner = AgentLoop::with_compact_config(
        Arc::new(RecordingSequenceProvider {
            requests: Arc::clone(&requests),
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("compact", serde_json::json!({})),
                text_response(
                    "{\"compacted_assistant\":\"这是压缩后的上下文，保留当前任务约束。\"}",
                ),
                text_response("继续完成"),
            ])),
        }),
        runtime_without_skills(),
        config.llm_config().clone(),
        config.agent_config().compact_config().clone(),
    );
    let (input, _outgoing_rx) = build_input();
    let active_thread = thread_with_history(vec![
        ChatMessage::new(
            ChatMessageRole::User,
            "需要大量上下文的历史问题".repeat(15),
            chrono::Utc::now(),
        ),
        ChatMessage::new(
            ChatMessageRole::Assistant,
            "需要大量上下文的历史回答".repeat(15),
            chrono::Utc::now(),
        ),
    ]);

    let output =
        run_turn_with_active_thread(&loop_runner, input, "system", "请继续", active_thread)
            .await
            .expect("loop should expose the compact tool");

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 3);
    assert!(
        captured_requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "compact")
    );
    assert!(
        captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
    assert!(
        captured_requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("超过 auto_compact 提前提醒阈值"))
    );
    let final_request = captured_requests
        .last()
        .expect("final post-compact request should exist");
    assert!(
        final_request
            .messages
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
    assert_eq!(output.events[0].kind, AgentLoopEventKind::ToolCall);
    assert_eq!(output.events[1].kind, AgentLoopEventKind::Compact);
    assert_eq!(output.events[2].kind, AgentLoopEventKind::TextOutput);
    assert!(
        final_request
            .messages
            .iter()
            .any(|message| message.content == "继续")
    );
    assert!(
        !final_request
            .messages
            .iter()
            .any(|message| message.content == "请继续")
    );
    assert_eq!(output.metadata["used_tool_name"], "compact");
    assert!(!output.persist_incoming_user);
    assert_eq!(output.reply, "继续完成");
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

async fn run_simple_turn(
    loop_runner: &AgentLoop,
    input: InfoContext,
    system_prompt: &str,
    user_message: &str,
) -> Result<openjarvis::agent::AgentLoopOutput> {
    let thread_context = build_thread_context_for_input(&input, system_prompt);
    let incoming = build_incoming_for_input(&input, user_message);
    loop_runner
        .run_v1(input.event_tx, &incoming, thread_context)
        .await
}

async fn run_turn_with_active_thread(
    loop_runner: &AgentLoop,
    input: InfoContext,
    system_prompt: &str,
    user_message: &str,
    active_thread: ConversationThread,
) -> Result<openjarvis::agent::AgentLoopOutput> {
    let mut thread_context = ThreadContext::from_conversation_thread(
        thread_context_locator_for_input(&input),
        active_thread,
    );
    let _ = thread_context.ensure_system_prompt_snapshot(system_prompt, chrono::Utc::now());
    let incoming = build_incoming_for_input(&input, user_message);
    loop_runner
        .run_v1(input.event_tx, &incoming, thread_context)
        .await
}

fn build_incoming_for_input(input: &InfoContext, user_message: &str) -> IncomingMessage {
    IncomingMessage {
        id: uuid::Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: input.channel.clone(),
        user_id: input.user_id.clone(),
        user_name: None,
        content: user_message.to_string(),
        external_thread_id: Some(input.compact_scope_key.external_thread_id.clone()),
        received_at: chrono::Utc::now(),
        metadata: serde_json::json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread_context_for_input(input: &InfoContext, system_prompt: &str) -> ThreadContext {
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(thread_context_locator_for_input(input), now);
    let _ = thread_context.ensure_system_prompt_snapshot(system_prompt, now);
    thread_context
}

fn thread_context_locator_for_input(input: &InfoContext) -> ThreadContextLocator {
    ThreadContextLocator::new(
        None,
        input.channel.clone(),
        input.user_id.clone(),
        input.compact_scope_key.external_thread_id.clone(),
        input.thread_id.clone(),
    )
}

fn runtime_without_skills() -> AgentRuntime {
    AgentRuntime::with_parts(
        Arc::new(HookRegistry::new()),
        Arc::new(ToolRegistry::with_skill_roots(Vec::new())),
    )
}

fn thread_with_history(history: Vec<ChatMessage>) -> ConversationThread {
    let now = chrono::Utc::now();
    let mut thread = ConversationThread::new("default", now);
    thread.store_turn(None, history, now, now);
    thread
}

fn build_input() -> (InfoContext, mpsc::Receiver<AgentDispatchEvent>) {
    build_input_for("thread_1", "thread_1")
}

fn build_input_for(
    thread_id: &str,
    external_thread_id: &str,
) -> (InfoContext, mpsc::Receiver<AgentDispatchEvent>) {
    let (tx, rx) = mpsc::channel(32);
    (
        InfoContext {
            channel: "feishu".to_string(),
            user_id: "ou_xxx".to_string(),
            thread_id: thread_id.to_string(),
            compact_scope_key: CompactScopeKey::new("feishu", "ou_xxx", external_thread_id),
            event_tx: AgentEventSender::new(
                tx,
                "feishu",
                Some(external_thread_id.to_string()),
                Some("msg_1".to_string()),
                ReplyTarget {
                    receive_id: "oc_xxx".to_string(),
                    receive_id_type: "chat_id".to_string(),
                },
                "session_1",
                "feishu",
                "ou_xxx",
                external_thread_id,
                thread_id,
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
