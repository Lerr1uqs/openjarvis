#[path = "support/mod.rs"]
mod support;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use clap::Parser;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentLoopEventKind, AgentRequest, AgentRuntime, AgentWorker,
        AgentWorkerEvent, AgentWorkerHandle, CommittedAgentDispatchItem, FinalizedAgentTurn,
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        agent_loop::{TOOL_EVENT_PREVIEW_MAX_CHARS, truncate_tool_message},
        empty_tool_input_schema,
    },
    channels::{Channel, ChannelRegistration},
    cli::OpenJarvisCli,
    command::CommandRegistry,
    compact::ContextBudgetEstimator,
    config::{AppConfig, BUILTIN_MCP_SERVER_NAME, DEFAULT_ASSISTANT_SYSTEM_PROMPT},
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    router::ChannelRouter,
    router::ChannelRouterBuilder,
    session::{MemorySessionStore, SessionKey, SessionManager, SessionStore},
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use support::{SessionManagerTestExt, ThreadTestExt};
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

struct RecordingChannel {
    name: &'static str,
    sent: Arc<Mutex<Vec<OutgoingMessage>>>,
    incoming_tx: Arc<Mutex<Option<mpsc::Sender<IncomingMessage>>>>,
}

struct MockAgentHarness {
    handle: AgentWorkerHandle,
    event_keepalive_tx: mpsc::Sender<AgentWorkerEvent>, // test-only: keeps the downstream event channel alive until shutdown.
}

#[derive(Clone)]
struct ObservedAgentRequest {
    request: AgentRequest,
    thread_context: Thread,
}

#[derive(Clone)]
struct TestCommittedEvent {
    kind: AgentLoopEventKind,
    content: String,
    metadata: serde_json::Value,
}

struct SequenceProvider {
    responses: Arc<Mutex<Vec<LLMResponse>>>,
}

struct LongResultTool;

#[async_trait]
impl LLMProvider for SequenceProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        Ok(self.responses.lock().await.remove(0))
    }
}

#[async_trait]
impl ToolHandler for LongResultTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__long_echo".to_string(),
            description: "Long tool result for router truncation tests".to_string(),
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

#[async_trait]
impl Channel for RecordingChannel {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn start(self: Arc<Self>, mut registration: ChannelRegistration) -> Result<()> {
        *self.incoming_tx.lock().await = Some(registration.incoming_tx);
        let sent = Arc::clone(&self.sent);
        tokio::spawn(async move {
            while let Some(message) = registration.outgoing_rx.recv().await {
                sent.lock().await.push(message);
            }
        });
        Ok(())
    }
}

async fn wait_for_test_shutdown(shutdown_rx: oneshot::Receiver<()>) {
    let _ = shutdown_rx.await;
}

struct ExternalMcpConfigFixture {
    root: PathBuf,
    config_path: PathBuf,
}

impl ExternalMcpConfigFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let config_path = root.join("config.yaml");
        Self { root, config_path }
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    fn write_mcp_json(&self, value: serde_json::Value) {
        let mcp_json_path = self.root.join("config/openjarvis/mcp.json");
        fs::create_dir_all(
            mcp_json_path
                .parent()
                .expect("mcp json parent path should exist"),
        )
        .expect("mcp json directory should be created");
        fs::write(
            &mcp_json_path,
            serde_json::to_string_pretty(&value).expect("mcp json should serialize"),
        )
        .expect("mcp json should be written");
    }
}

impl Drop for ExternalMcpConfigFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn router_ignores_duplicate_messages() {
    let agent = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("reply")))
        .system_prompt("system")
        .build()
        .expect("worker should build");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .message_dedup_enabled(true)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .clone()
        .expect("channel sender should be captured");
    let incoming = build_incoming();

    let driver = async {
        channel_tx
            .send(incoming.clone())
            .await
            .expect("first message should be sent");
        channel_tx
            .send(incoming)
            .await
            .expect("duplicate message should be sent");

        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("outgoing message should be recorded");

        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].content, "reply");
    assert_eq!(recorded[0].metadata["event_kind"], "TextOutput");
    assert_eq!(recorded[0].metadata["session_channel"], "feishu");
    assert_eq!(recorded[0].metadata["session_user_id"], "ou_xxx");
    assert_eq!(
        recorded[0].metadata["session_external_thread_id"],
        "default"
    );
    assert_eq!(recorded[0].reply_to_message_id.as_deref(), Some("msg_1"));
    assert!(recorded[0].attachments.is_empty());
}

#[tokio::test]
async fn router_parses_attachment_syntax_before_channel_dispatch() {
    let agent = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new(
            "这是生成的图片\n#!openjarvis[image:/tmp/router-image.png]",
        )))
        .system_prompt("system")
        .build()
        .expect("worker should build");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .clone()
        .expect("channel sender should be captured");

    let driver = async {
        channel_tx
            .send(build_incoming())
            .await
            .expect("incoming message should be sent");

        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("outgoing message should be recorded");

        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "这是生成的图片\n#!openjarvis[image:/tmp/router-image.png]"
    );
    assert_eq!(recorded[0].attachments.len(), 1);
    assert_eq!(recorded[0].attachments[0].path, "/tmp/router-image.png");
}

#[tokio::test]
async fn router_preserves_full_outgoing_text_before_channel_dispatch() {
    // 测试场景: 普通 assistant 回复发给 channel 时必须保持原文，router 不能截断正常文本回复。
    let agent = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("unused")))
        .system_prompt("system")
        .build()
        .expect("worker should build");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    router
        .dispatch_outgoing(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: "feishu".to_string(),
            content: "A".repeat(5_000),
            external_thread_id: Some("thread_truncate".to_string()),
            metadata: json!({
                "event_kind": "TextOutput",
                "summary": "B".repeat(5_000),
                "nested": {
                    "items": [
                        "C".repeat(5_000)
                    ]
                }
            }),
            reply_to_message_id: Some("msg_truncate".to_string()),
            attachments: Vec::new(),
            target: ReplyTarget {
                receive_id: "oc_truncate".to_string(),
                receive_id_type: "chat_id".to_string(),
            },
        })
        .await
        .expect("router should dispatch outgoing message");

    timeout(Duration::from_millis(500), async {
        loop {
            if sent.lock().await.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("outgoing message should be recorded");

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].content, "A".repeat(5_000));
    assert_eq!(
        recorded[0].metadata["summary"]
            .as_str()
            .expect("summary should remain a string")
            .chars()
            .count(),
        5_000
    );
    assert!(
        !recorded[0].metadata["summary"]
            .as_str()
            .expect("summary should remain a string")
            .contains("...(truncated, total_chars=5000)")
    );
    assert_eq!(
        recorded[0].metadata["nested"]["items"][0]
            .as_str()
            .expect("nested item should remain a string")
            .chars()
            .count(),
        5_000
    );
    assert_eq!(
        recorded[0].reply_to_message_id.as_deref(),
        Some("msg_truncate")
    );
}

#[tokio::test]
async fn router_delivers_truncated_tool_events_but_full_final_reply() {
    // 测试场景: 真实 agent loop 产生的 tool_call/tool_result 事件发给 channel 时应被截断，但最终 assistant 回复必须保留全量。
    let runtime = AgentRuntime::new();
    runtime
        .tools()
        .register(Arc::new(LongResultTool))
        .await
        .expect("long result tool should register");
    let long_arguments = json!({
        "path": "X".repeat(128),
    });
    let agent = AgentWorker::with_runtime(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                LLMResponse {
                    message: Some(ChatMessage::new(
                        ChatMessageRole::Assistant,
                        "开始执行",
                        Utc::now(),
                    )),
                    tool_calls: vec![openjarvis::llm::LLMToolCall {
                        id: "call_router_long_1".to_string(),
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
            ])),
        }),
        "system",
        runtime,
    );
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .clone()
        .expect("channel sender should be captured");

    let driver = async {
        channel_tx
            .send(build_incoming())
            .await
            .expect("incoming message should be sent");

        timeout(Duration::from_millis(800), async {
            loop {
                if sent.lock().await.len() == 4 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("tool events and final reply should be recorded");

        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].content, "开始执行");
    assert_eq!(recorded[0].metadata["event_kind"], "TextOutput");
    assert_eq!(
        recorded[1].content,
        format!(
            "[openjarvis][tool_call] demo__long_echo {}",
            truncate_tool_message(&long_arguments.to_string(), TOOL_EVENT_PREVIEW_MAX_CHARS)
        )
    );
    assert_eq!(recorded[1].metadata["event_kind"], "ToolCall");
    assert_eq!(
        recorded[2].content,
        format!(
            "[openjarvis][tool_result] {}",
            truncate_tool_message(&"R".repeat(128), TOOL_EVENT_PREVIEW_MAX_CHARS)
        )
    );
    assert_eq!(recorded[2].metadata["event_kind"], "ToolResult");
    assert_eq!(recorded[3].content, "done");
    assert_eq!(recorded[3].metadata["event_kind"], "TextOutput");
}

#[test]
fn router_builder_requires_agent_or_handle() {
    // 测试场景: builder 在缺少 agent/handle 时应拒绝构建，避免 router 处于不可运行状态。
    let error = ChannelRouterBuilder::new()
        .build()
        .err()
        .expect("router builder without agent should fail");

    assert!(error.to_string().contains("agent worker"));
}

#[tokio::test]
async fn router_external_channel_message_can_trigger_builtin_mcp_when_flag_enabled() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "--builtin-mcp"]);
    let mut config = AppConfig::default();
    if cli.builtin_mcp {
        config
            .enable_builtin_mcp(openjarvis_bin())
            .expect("builtin mcp should be enabled");
    }

    let runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("runtime should build with builtin MCP");
    let servers = runtime.tools().mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, BUILTIN_MCP_SERVER_NAME);
    assert_eq!(servers[0].tool_count, 3);

    let agent = AgentWorker::with_runtime(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("load_toolset", json!({ "name": BUILTIN_MCP_SERVER_NAME })),
                tool_only_response(
                    "mcp__builtin_demo_stdio__echo",
                    json!({ "text": "channel hello" }),
                ),
                text_response("builtin mcp finished"),
            ])),
        }),
        DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        runtime,
    );
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_builtin_mcp",
        "ou_builtin_mcp",
        Some("thread_builtin_mcp"),
        "请调用内置 MCP",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("external channel message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(1500), async {
            loop {
                if sent.lock().await.len() == 5 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("builtin MCP outgoing messages should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");

    let recorded = sent.lock().await.clone();
    assert_eq!(recorded.len(), 5);
    assert!(
        recorded[0]
            .content
            .contains("[openjarvis][tool_call] load_toolset")
    );
    assert_eq!(recorded[0].metadata["event_kind"], "ToolCall");
    assert!(
        recorded[2]
            .content
            .contains("[openjarvis][tool_call] mcp__builtin_demo_stdio__echo")
    );
    assert_eq!(recorded[2].metadata["event_kind"], "ToolCall");
    assert!(recorded[3].content.contains("[demo:stdio] channel hello"));
    assert_eq!(recorded[3].metadata["event_kind"], "ToolResult");
    assert_eq!(recorded[4].content, "builtin mcp finished");
    assert_eq!(recorded[4].metadata["event_kind"], "TextOutput");
}

#[tokio::test]
async fn router_external_channel_message_can_trigger_mcp_loaded_from_external_json_file() {
    let fixture = ExternalMcpConfigFixture::new("openjarvis-router-mcp-json");
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "file_demo_stdio": {
                "command": openjarvis_bin(),
                "args": ["internal-mcp", "demo-stdio"]
            }
        }
    }));

    let config =
        AppConfig::from_path(fixture.config_path()).expect("external mcp json should load");
    let runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("runtime should build with file-loaded MCP");
    let servers = runtime.tools().mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "file_demo_stdio");
    assert_eq!(servers[0].tool_count, 3);

    let agent = AgentWorker::with_runtime(
        Arc::new(SequenceProvider {
            responses: Arc::new(Mutex::new(vec![
                tool_only_response("load_toolset", json!({ "name": "file_demo_stdio" })),
                tool_only_response(
                    "mcp__file_demo_stdio__echo",
                    json!({ "text": "config hello" }),
                ),
                text_response("config mcp finished"),
            ])),
        }),
        DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        runtime,
    );
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_config_mcp",
        "ou_config_mcp",
        Some("thread_config_mcp"),
        "请调用配置文件里的 MCP",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("external channel message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(1500), async {
            loop {
                if sent.lock().await.len() == 5 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("file-loaded MCP outgoing messages should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");

    let recorded = sent.lock().await.clone();
    assert_eq!(recorded.len(), 5);
    assert!(
        recorded[0]
            .content
            .contains("[openjarvis][tool_call] load_toolset")
    );
    assert_eq!(recorded[0].metadata["event_kind"], "ToolCall");
    assert!(
        recorded[2]
            .content
            .contains("[openjarvis][tool_call] mcp__file_demo_stdio__echo")
    );
    assert_eq!(recorded[2].metadata["event_kind"], "ToolCall");
    assert!(recorded[3].content.contains("[demo:stdio] config hello"));
    assert_eq!(recorded[3].metadata["event_kind"], "ToolResult");
    assert_eq!(recorded[4].content, "config mcp finished");
    assert_eq!(recorded[4].metadata["event_kind"], "TextOutput");
}

#[tokio::test]
async fn router_stores_two_turns_for_same_session_thread_with_mock_agent() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let agent_harness = build_mock_agent_handle(Arc::clone(&observed_requests));
    let event_keepalive_tx = agent_harness.event_keepalive_tx; // test-only: prevents the mock downstream channel from looking crashed.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(agent_harness.handle)
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let first_incoming = build_incoming_with(
        "msg_turn_1",
        "ou_shared",
        Some("thread_shared"),
        "first question",
    );
    let second_incoming = build_incoming_with(
        "msg_turn_2",
        "ou_shared",
        Some("thread_shared"),
        "second question",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(first_incoming)
            .await
            .expect("first message should be sent");
        channel_tx
            .send(second_incoming)
            .await
            .expect("second message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 4 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all outgoing messages should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_keepalive_tx);

    let observed_requests = observed_requests.lock().await.clone();
    let locator = observed_requests[0].locator.clone();
    let history = router
        .sessions()
        .load_non_system_messages(&locator)
        .await
        .expect("history should load");
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_shared".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");
    let recorded = sent.lock().await.clone();

    assert_eq!(observed_requests.len(), 2);
    assert_eq!(observed_requests[0].incoming.content, "first question");
    assert_eq!(observed_requests[1].incoming.content, "second question");
    assert_eq!(observed_requests[0].locator.channel, "feishu");
    assert_eq!(observed_requests[0].locator.user_id, "ou_shared");
    assert_eq!(
        observed_requests[0].locator.external_thread_id,
        "thread_shared"
    );
    assert_eq!(observed_requests[1].locator.channel, "feishu");
    assert_eq!(observed_requests[1].locator.user_id, "ou_shared");
    assert_eq!(
        observed_requests[1].locator.external_thread_id,
        "thread_shared"
    );
    assert_eq!(
        observed_requests[0].locator.session_id,
        observed_requests[1].locator.session_id
    );
    assert_eq!(
        observed_requests[0].locator.thread_id,
        observed_requests[1].locator.thread_id
    );

    assert_eq!(thread.non_system_messages().len(), 6);
    assert_eq!(thread.locator.external_thread_id, "thread_shared");
    assert_eq!(history.len(), 6);
    assert_eq!(history[0].content, "first question");
    assert_eq!(history[1].content, "reply-first");
    assert_eq!(history[5].content, "reply-second");
    assert_eq!(thread.non_system_messages()[0].content, "first question");
    assert_eq!(thread.non_system_messages()[1].content, "reply-first");
    assert_eq!(thread.non_system_messages()[2].content, "second question");
    assert_eq!(
        thread.non_system_messages()[3].tool_calls[0].id,
        "call_mock_1"
    );
    assert_eq!(
        thread.non_system_messages()[4].tool_call_id.as_deref(),
        Some("call_mock_1")
    );
    assert_eq!(thread.non_system_messages()[5].content, "reply-second");

    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].content, "reply-first");
    assert!(recorded[1].content.contains("[openjarvis][tool_call]"));
    assert!(recorded[2].content.contains("[openjarvis][tool_result]"));
    assert_eq!(recorded[3].content, "reply-second");
}

#[tokio::test]
async fn router_preserves_large_history_before_next_turn() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::<ObservedAgentRequest>::new()));
    let agent_harness = build_truncation_mock_agent_handle(Arc::clone(&observed_requests));
    let event_keepalive_tx = agent_harness.event_keepalive_tx; // test-only: prevents the mock downstream channel from looking crashed.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(agent_harness.handle)
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let first_incoming = build_incoming_with(
        "msg_truncation_1",
        "ou_truncation",
        Some("thread_truncation"),
        "trigger many replies",
    );
    let second_incoming = build_incoming_with(
        "msg_truncation_2",
        "ou_truncation",
        Some("thread_truncation"),
        "check history after truncation",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(first_incoming)
            .await
            .expect("first truncation message should be sent");
        channel_tx
            .send(second_incoming)
            .await
            .expect("second truncation message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 7 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all truncation outgoing messages should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_keepalive_tx);

    let observed_requests = observed_requests.lock().await.clone();
    let locator = observed_requests[0].request.locator.clone();
    let history = router
        .sessions()
        .load_non_system_messages(&locator)
        .await
        .expect("history should load");
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_truncation".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");
    let recorded = sent.lock().await.clone();

    assert_eq!(observed_requests.len(), 2);
    assert_eq!(
        observed_requests[1]
            .thread_context
            .non_system_messages()
            .len(),
        7
    );
    assert_eq!(
        observed_requests[1]
            .thread_context
            .non_system_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "trigger many replies".to_string(),
            "message_1".to_string(),
            "message_2".to_string(),
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );

    assert_eq!(thread.non_system_messages().len(), 9);
    assert_eq!(thread.locator.external_thread_id, "thread_truncation");
    assert_eq!(
        thread.non_system_messages()[..7]
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "trigger many replies".to_string(),
            "message_1".to_string(),
            "message_2".to_string(),
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );
    assert_eq!(
        thread.non_system_messages()[7..]
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "check history after truncation".to_string(),
            "final-after-truncation".to_string(),
        ]
    );
    assert_eq!(history.len(), 9);
    assert_eq!(history[0].content, "trigger many replies");
    assert_eq!(history[1].content, "message_1");
    assert_eq!(history[6].content, "message_6");
    assert_eq!(history[7].content, "check history after truncation");
    assert_eq!(history[8].content, "final-after-truncation");

    assert_eq!(recorded.len(), 7);
    assert_eq!(recorded[0].content, "message_1");
    assert_eq!(recorded[5].content, "message_6");
    assert_eq!(recorded[6].content, "final-after-truncation");
}

#[tokio::test]
async fn router_short_circuits_registered_command_without_session_or_agent() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_command_echo",
        "ou_command",
        Some("thread_command"),
        "/echo keep everything",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("command message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("command reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_command".to_string(),
        })
        .await
        .expect("command should still resolve and persist the target thread");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][echo][SUCCESS]: keep everything"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "echo");
    assert_eq!(recorded[0].metadata["command_status"], "SUCCESS");
    assert!(request_rx.try_recv().is_err());
    assert_eq!(session.threads.len(), 1);
    assert!(
        session
            .threads
            .values()
            .all(|thread| thread.non_system_messages().is_empty())
    );
}

#[tokio::test]
async fn router_returns_failed_reply_for_unknown_command_without_session_or_agent() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_command_unknown",
        "ou_unknown",
        Some("thread_unknown"),
        "/missing payload",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("unknown command should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unknown command reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_unknown".to_string(),
        })
        .await
        .expect("unknown command should still resolve and persist the target thread");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][missing][FAILED]: unknown command"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "missing");
    assert_eq!(recorded[0].metadata["command_status"], "FAILED");
    assert!(request_rx.try_recv().is_err());
    assert_eq!(session.threads.len(), 1);
    assert!(
        session
            .threads
            .values()
            .all(|thread| thread.non_system_messages().is_empty())
    );
}

#[tokio::test]
async fn router_removed_auto_compact_command_returns_unknown_reply_without_agent_dispatch() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let commands = CommandRegistry::with_builtin_commands();
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .command_registry(commands)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_command_thread",
        "ou_thread_command",
        Some("thread_runtime"),
        "/auto-compact on",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("thread-scoped command should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("thread-scoped command reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_thread_command".to_string(),
        })
        .await
        .expect("thread-scoped command should create session state");
    let thread = session
        .threads
        .values()
        .next()
        .expect("thread should be stored after command");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][auto-compact][FAILED]: unknown command"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "auto-compact");
    assert_eq!(recorded[0].metadata["command_status"], "FAILED");
    assert!(request_rx.try_recv().is_err());
    assert!(!thread.auto_compact_enabled(false));
    assert!(thread.non_system_messages().is_empty());
}

#[tokio::test]
async fn router_context_command_returns_summary_without_agent_dispatch() {
    // 测试场景: `/context` 应直接在命令层返回摘要，不触发 agent dispatch，也不破坏现有线程历史。
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let commands = CommandRegistry::with_builtin_commands();
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let sessions = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("shared store session manager should build");
    let seed_incoming = build_incoming_with(
        "msg_context_seed",
        "ou_thread_context",
        Some("thread_context"),
        "seed",
    );
    let locator = sessions
        .load_or_create_thread(&seed_incoming)
        .await
        .expect("thread should resolve before context inspection");
    let now = Utc::now();
    let mut seeded_thread = Thread::new(ThreadContextLocator::from(&locator), now);
    seeded_thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    seeded_thread.commit_test_turn(
        seed_incoming.external_message_id.clone(),
        vec![
            ChatMessage::new(ChatMessageRole::User, "context summary user", now),
            ChatMessage::new(ChatMessageRole::Assistant, "context summary assistant", now),
        ],
        now,
        now,
    );
    let expected_thread = seeded_thread.clone();
    sessions
        .store_thread_context(&locator, seeded_thread, now)
        .await
        .expect("seed thread state should store");

    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .command_registry(commands)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_context_command",
        "ou_thread_context",
        Some("thread_context"),
        "/context",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("context command should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("context command reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_thread_context".to_string(),
        })
        .await
        .expect("context command should keep session state");
    let thread = session
        .threads
        .values()
        .next()
        .expect("thread should remain addressable after context inspection");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        format!(
            "[Command][context][SUCCESS]: {}",
            expected_context_summary(&expected_thread)
        )
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "context");
    assert_eq!(recorded[0].metadata["command_status"], "SUCCESS");
    assert!(request_rx.try_recv().is_err());
    assert_eq!(thread.messages(), expected_thread.messages());
    assert_eq!(thread.load_toolsets(), expected_thread.load_toolsets());
}

#[tokio::test]
async fn router_clear_command_resets_persisted_thread_context_without_agent_dispatch() {
    // 测试场景: /clear 应清空当前线程的持久化历史和线程状态，同时不触发 agent dispatch。
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let commands = CommandRegistry::with_builtin_commands();
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let sessions = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("shared store session manager should build");
    let seed_incoming = build_incoming_with(
        "msg_clear_seed",
        "ou_thread_clear",
        Some("thread_clear"),
        "seed",
    );
    let locator = sessions
        .load_or_create_thread(&seed_incoming)
        .await
        .expect("thread should resolve before clear");
    let now = Utc::now();
    let mut seeded_thread = Thread::new(ThreadContextLocator::from(&locator), now);
    seeded_thread.enable_auto_compact();
    seeded_thread.commit_test_turn_with_state(
        seed_incoming.external_message_id.clone(),
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "需要被清空的历史",
            now,
        )],
        now,
        now,
        vec!["demo".to_string()],
        Vec::new(),
    );
    sessions
        .store_thread_context(&locator, seeded_thread, now)
        .await
        .expect("seed thread state should store");

    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .command_registry(commands)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_command_clear",
        "ou_thread_clear",
        Some("thread_clear"),
        "/clear",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("clear command should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("clear command reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_thread_clear".to_string(),
        })
        .await
        .expect("clear command should keep session state");
    let thread = session
        .threads
        .values()
        .next()
        .expect("thread should remain addressable after clear");
    let restored_reader = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("restored reader should build");
    let restored = restored_reader
        .load_thread_context(&locator)
        .await
        .expect("restored thread should load")
        .expect("restored thread should exist");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][clear][SUCCESS]: cleared current thread `thread_clear`; all chat messages and thread-scoped runtime state have been reset"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "clear");
    assert_eq!(recorded[0].metadata["command_status"], "SUCCESS");
    assert!(request_rx.try_recv().is_err());
    assert!(thread.non_system_messages().is_empty());
    assert!(thread.load_toolsets().is_empty());
    assert!(!thread.auto_compact_enabled(false));
    assert!(restored.non_system_messages().is_empty());
    assert!(restored.load_toolsets().is_empty());
    assert!(!restored.auto_compact_enabled(false));
}

#[tokio::test]
async fn router_clear_command_returns_running_while_thread_is_pending() {
    // 测试场景: 当前 thread 仍在 agent loop 中时，/clear 不能竞争 live thread lock，
    // 必须直接返回 running，并保持已有 thread 状态不变。
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive during the command test.
    let commands = CommandRegistry::with_builtin_commands();
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let sessions = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("shared store session manager should build");
    let seed_incoming = build_incoming_with(
        "msg_clear_running_seed",
        "ou_thread_clear_running",
        Some("thread_clear_running"),
        "seed",
    );
    let locator = sessions
        .load_or_create_thread(&seed_incoming)
        .await
        .expect("thread should resolve before clear");
    let now = Utc::now();
    let mut seeded_thread = Thread::new(ThreadContextLocator::from(&locator), now);
    seeded_thread.commit_test_turn(
        seed_incoming.external_message_id.clone(),
        vec![
            ChatMessage::new(ChatMessageRole::User, "persisted before running", now),
            ChatMessage::new(ChatMessageRole::Assistant, "persisted reply", now),
        ],
        now,
        now,
    );
    sessions
        .store_thread_context(&locator, seeded_thread, now)
        .await
        .expect("seed thread state should store");

    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .command_registry(commands)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let running_incoming = build_incoming_with(
        "msg_thread_running",
        "ou_thread_clear_running",
        Some("thread_clear_running"),
        "continue running",
    );
    let clear_incoming = build_incoming_with(
        "msg_clear_running_command",
        "ou_thread_clear_running",
        Some("thread_clear_running"),
        "/clear",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(running_incoming)
            .await
            .expect("running message should be sent");
        channel_tx
            .send(clear_incoming)
            .await
            .expect("clear command should be sent");
    });

    let driver = async {
        let dispatched_request = timeout(Duration::from_millis(500), request_rx.recv())
            .await
            .expect("running request should dispatch")
            .expect("agent request should exist");
        assert_eq!(dispatched_request.incoming.content, "continue running");

        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("clear running reply should be recorded");

        assert!(request_rx.try_recv().is_err());
        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_tx);

    let recorded = sent.lock().await.clone();
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_thread_clear_running".to_string(),
        })
        .await
        .expect("session should remain accessible");
    let thread = session
        .threads
        .values()
        .next()
        .expect("thread should remain addressable after running clear");
    let restored_reader = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("restored reader should build");
    let restored = restored_reader
        .load_thread_context(&locator)
        .await
        .expect("restored thread should load")
        .expect("restored thread should exist");

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][clear][FAILED]: current thread is running; /clear is unavailable until the active agent turn completes"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "clear");
    assert_eq!(recorded[0].metadata["command_status"], "FAILED");
    assert_eq!(thread.non_system_messages().len(), 2);
    assert_eq!(
        thread.non_system_messages()[0].content,
        "persisted before running"
    );
    assert_eq!(thread.non_system_messages()[1].content, "persisted reply");
    assert_eq!(restored.non_system_messages().len(), 2);
    assert_eq!(
        restored.non_system_messages()[0].content,
        "persisted before running"
    );
    assert_eq!(restored.non_system_messages()[1].content, "persisted reply");
}

#[tokio::test]
async fn router_failed_turn_replies_with_full_error_chain() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, _request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8);
    let event_keepalive_tx = event_tx.clone(); // test-only: keeps the downstream event channel alive until explicit shutdown.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let incoming = build_incoming_with(
        "msg_agent_error",
        "ou_agent_error",
        Some("thread_agent_error"),
        "why did provider fail",
    );
    let locator = router
        .sessions()
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let locator_for_send = locator.clone();
    let sessions = router.sessions().clone();

    let send_task = tokio::spawn(async move {
        let request = AgentRequest {
            locator: locator_for_send.clone(),
            incoming: incoming.clone(),
            sessions,
        };
        let mut thread_context = request
            .sessions
            .lock_thread_context(&request.locator, request.incoming.received_at)
            .await
            .expect("failed turn should lock");
        thread_context
            .begin_turn(
                request.incoming.external_message_id.clone(),
                request.incoming.received_at,
            )
            .expect("failed turn should start");
        let mut reply_to_source = true;
        let user_message = ChatMessage::new(
            ChatMessageRole::User,
            request.incoming.content.clone(),
            request.incoming.received_at,
        );
        thread_context
            .push_message(user_message)
            .await
            .expect("failed turn user message should commit");
        let failure_message = ChatMessage::new(
            ChatMessageRole::Assistant,
            "[openjarvis][agent_error] failed to call llm provider `openai_compatible` model `demo-model` at `https://provider.test/v1`: provider said 429: rate limit exceeded",
            Utc::now(),
        );
        let failure_event = TestCommittedEvent {
            kind: AgentLoopEventKind::TextOutput,
            content: failure_message.content.clone(),
            metadata: json!({
                "source": "mock_failed_agent",
                "is_final": true,
                "is_error": true,
            }),
        };
        thread_context
            .push_message(failure_message.clone())
            .await
            .expect("failed turn reply should commit");
        send_committed_message(
            &event_tx,
            &request,
            build_dispatch_batch(&request, &mut reply_to_source, &[failure_event]),
            failure_message.created_at,
        )
        .await;
        let turn = thread_context
            .finalize_turn_failure(
                "failed to call llm provider `openai_compatible` model `demo-model` at `https://provider.test/v1`: provider said 429: rate limit exceeded",
                Utc::now(),
            )
            .expect("failed turn should finalize");
        request
            .sessions
            .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
            .await
            .expect("failed turn should persist");
        event_tx
            .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                locator: request.locator.clone(),
                turn,
            }))
            .await
            .expect("failed turn should be sent");
        event_tx
            .send(AgentWorkerEvent::RequestCompleted(
                openjarvis::agent::CompletedAgentRequest {
                    locator: request.locator.clone(),
                    completed_at: Utc::now(),
                },
            ))
            .await
            .expect("failed request should complete");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("agent error reply should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_keepalive_tx);

    let recorded = sent.lock().await.clone();
    let history = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_agent_error".to_string(),
        })
        .await
        .expect("session should exist");

    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].content.contains("[openjarvis][agent_error]"));
    assert!(
        recorded[0]
            .content
            .contains("provider said 429: rate limit exceeded")
    );
    assert_eq!(recorded[0].metadata["event_kind"], "TextOutput");
    assert_eq!(recorded[0].metadata["event_metadata"]["is_error"], true);
    assert_eq!(history.threads.len(), 1);
    let thread = history
        .threads
        .values()
        .next()
        .expect("thread should exist after failed commit");
    assert_eq!(thread.non_system_messages().len(), 2);
    assert_eq!(
        thread.non_system_messages()[0].content,
        "why did provider fail"
    );
    assert!(
        thread.non_system_messages()[1]
            .content
            .contains("provider said 429: rate limit exceeded")
    );
}

#[tokio::test]
async fn router_does_not_persist_dispatch_cursor_after_sending_item() {
    // 测试场景: router 发送单条 committed event 成功后，不会再向 session 回写任何 dispatch cursor。
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let (request_tx, _request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8);
    let event_keepalive_tx = event_tx.clone(); // test-only: keeps router event stream open until shutdown.
    let sessions = SessionManager::new();
    let mut router = ChannelRouterBuilder::new()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(sessions.clone())
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let incoming = build_incoming_with(
        "msg_dispatch_ack",
        "ou_dispatch_ack",
        Some("thread_dispatch_ack"),
        "hello",
    );
    let locator = router
        .sessions()
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let locator_for_send = locator.clone();
    let sessions = router.sessions().clone();

    let send_task = tokio::spawn(async move {
        let request = AgentRequest {
            locator: locator_for_send.clone(),
            incoming: incoming.clone(),
            sessions,
        };
        let mut thread_context = request
            .sessions
            .lock_thread_context(&request.locator, request.incoming.received_at)
            .await
            .expect("thread should lock");
        thread_context
            .begin_turn(
                request.incoming.external_message_id.clone(),
                request.incoming.received_at,
            )
            .expect("turn should start");
        let mut reply_to_source = true;
        let user_message = ChatMessage::new(
            ChatMessageRole::User,
            request.incoming.content.clone(),
            request.incoming.received_at,
        );
        thread_context
            .push_message(user_message)
            .await
            .expect("user message should commit");
        let assistant_message = ChatMessage::new(ChatMessageRole::Assistant, "reply", Utc::now());
        let assistant_event = TestCommittedEvent {
            kind: AgentLoopEventKind::TextOutput,
            content: "reply".to_string(),
            metadata: json!({
                "source": "router_ack_test",
                "is_final": true,
            }),
        };
        thread_context
            .push_message(assistant_message.clone())
            .await
            .expect("assistant message should commit");
        send_committed_message(
            &event_tx,
            &request,
            build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
            assistant_message.created_at,
        )
        .await;
        let turn = thread_context
            .finalize_turn_success("reply", Utc::now())
            .expect("turn should finalize");
        request
            .sessions
            .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
            .await
            .expect("turn should persist");
        event_tx
            .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                locator: request.locator.clone(),
                turn,
            }))
            .await
            .expect("finalized turn should send");
        event_tx
            .send(AgentWorkerEvent::RequestCompleted(
                openjarvis::agent::CompletedAgentRequest {
                    locator: request.locator.clone(),
                    completed_at: Utc::now(),
                },
            ))
            .await
            .expect("request should complete");
    });

    let driver = tokio::spawn(async move {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("router should send the dispatch item");
        shutdown_tx.send(()).expect("shutdown should send");
    });

    router
        .run_until_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .expect("router should exit cleanly");
    send_task.await.expect("send task should finish");
    driver.await.expect("driver should finish");
    drop(event_keepalive_tx);

    let stored_thread = router
        .sessions()
        .load_thread_context(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    let serialized = serde_json::to_value(&stored_thread).expect("thread should serialize");
    assert!(serialized["state"].get("dispatch").is_none());
    assert_eq!(
        stored_thread
            .non_system_messages()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "reply"]
    );
}

#[tokio::test]
async fn router_command_message_does_not_enter_existing_session() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let agent_harness = build_single_turn_mock_agent_handle(Arc::clone(&observed_requests));
    let event_keepalive_tx = agent_harness.event_keepalive_tx; // test-only: prevents the mock downstream channel from looking crashed.
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(agent_harness.handle)
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let first_incoming = build_incoming_with(
        "msg_normal_before_command",
        "ou_mix",
        Some("thread_mix"),
        "normal question",
    );
    let second_incoming = build_incoming_with(
        "msg_command_after_normal",
        "ou_mix",
        Some("thread_mix"),
        "/echo keep out of session",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(first_incoming)
            .await
            .expect("normal message should be sent");
        channel_tx
            .send(second_incoming)
            .await
            .expect("command message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mixed outgoing messages should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_keepalive_tx);

    let observed_requests = observed_requests.lock().await.clone();
    let locator = observed_requests[0].locator.clone();
    let history = router
        .sessions()
        .load_non_system_messages(&locator)
        .await
        .expect("history should load");
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_mix".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");
    let recorded = sent.lock().await.clone();

    assert_eq!(observed_requests.len(), 1);
    assert_eq!(thread.non_system_messages().len(), 2);
    assert_eq!(
        thread
            .non_system_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec!["normal question".to_string(), "reply-single".to_string()]
    );
    assert_eq!(
        history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec!["normal question".to_string(), "reply-single".to_string()]
    );
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded
            .iter()
            .any(|message| message.content == "reply-single")
    );
    assert!(
        recorded
            .iter()
            .any(|message| { message.content == "[Command][echo][SUCCESS]: keep out of session" })
    );
}

#[tokio::test]
async fn router_completed_turn_can_skip_prepending_incoming_user_after_compact() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let agent_harness = build_compact_mock_agent_handle();
    let event_keepalive_tx = agent_harness.event_keepalive_tx;
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::builder()
        .agent_handle(agent_harness.handle)
        .session_manager(sessions)
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let incoming = build_incoming_with(
        "msg_compact_router",
        "ou_compact_router",
        Some("thread_compact_router"),
        "被 compact 的旧问题",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(incoming)
            .await
            .expect("compact incoming message should be sent");
    });

    let driver = async {
        timeout(Duration::from_millis(500), async {
            loop {
                if sent.lock().await.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("compact outgoing message should be recorded");

        send_task.await.expect("sender task should complete");
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
        Ok::<(), anyhow::Error>(())
    };
    let (router_result, driver_result) = tokio::join!(
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
        driver
    );
    driver_result.expect("driver task should complete");
    router_result.expect("router loop should exit cleanly");
    drop(event_keepalive_tx);

    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_compact_router".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .values()
        .next()
        .expect("thread should exist");
    let history = thread.non_system_messages();

    assert_eq!(thread.non_system_messages().len(), 3);
    assert_eq!(
        thread.non_system_messages()[0].content,
        "这是压缩后的上下文"
    );
    assert_eq!(thread.non_system_messages()[1].content, "继续");
    assert_eq!(
        thread.non_system_messages()[2].content,
        "reply-after-compact"
    );
    assert_eq!(
        history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "这是压缩后的上下文".to_string(),
            "继续".to_string(),
            "reply-after-compact".to_string(),
        ]
    );
}

fn build_mock_agent_handle(observed_requests: Arc<Mutex<Vec<AgentRequest>>>) -> MockAgentHarness {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);
    let event_keepalive_tx = event_tx.clone(); // test-only: keeps the downstream event channel open until explicit shutdown.

    spawn_mock_agent_loop(observed_requests, event_tx, request_rx);

    MockAgentHarness {
        handle: AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        event_keepalive_tx,
    }
}

fn build_single_turn_mock_agent_handle(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
) -> MockAgentHarness {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);
    let event_keepalive_tx = event_tx.clone(); // test-only: keeps the downstream event channel open until explicit shutdown.

    spawn_single_turn_mock_agent_loop(observed_requests, event_tx, request_rx);

    MockAgentHarness {
        handle: AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        event_keepalive_tx,
    }
}

fn build_compact_mock_agent_handle() -> MockAgentHarness {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);
    let event_keepalive_tx = event_tx.clone();

    spawn_compact_mock_agent_loop(event_tx, request_rx);

    MockAgentHarness {
        handle: AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        event_keepalive_tx,
    }
}

fn build_truncation_mock_agent_handle(
    observed_requests: Arc<Mutex<Vec<ObservedAgentRequest>>>,
) -> MockAgentHarness {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);
    let event_keepalive_tx = event_tx.clone(); // test-only: keeps the downstream event channel open until explicit shutdown.

    spawn_truncation_mock_agent_loop(observed_requests, event_tx, request_rx);

    MockAgentHarness {
        handle: AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        event_keepalive_tx,
    }
}

fn spawn_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for step in 0..2 {
            let request = request_rx
                .recv()
                .await
                .expect("mock agent should receive scripted request");
            observed_requests.lock().await.push(request.clone());

            match step {
                0 => {
                    let mut thread_context = request
                        .sessions
                        .lock_thread_context(&request.locator, request.incoming.received_at)
                        .await
                        .expect("first mock turn should lock");
                    thread_context
                        .begin_turn(
                            request.incoming.external_message_id.clone(),
                            request.incoming.received_at,
                        )
                        .expect("first mock turn should start");
                    let mut reply_to_source = true;
                    thread_context
                        .push_message(ChatMessage::new(
                            ChatMessageRole::User,
                            request.incoming.content.clone(),
                            request.incoming.received_at,
                        ))
                        .await
                        .expect("first mock turn user message should commit");
                    let assistant_message =
                        ChatMessage::new(ChatMessageRole::Assistant, "reply-first", Utc::now());
                    thread_context
                        .push_message(assistant_message.clone())
                        .await
                        .expect("first mock reply should commit");
                    let assistant_event = TestCommittedEvent {
                        kind: AgentLoopEventKind::TextOutput,
                        content: "reply-first".to_string(),
                        metadata: json!({
                            "source": "mock_agent",
                            "is_final": true,
                        }),
                    };
                    send_committed_message(
                        &event_tx,
                        &request,
                        build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
                        assistant_message.created_at,
                    )
                    .await;
                    let turn = thread_context
                        .finalize_turn_success("reply-first", Utc::now())
                        .expect("first mock turn should finalize");
                    request
                        .sessions
                        .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
                        .await
                        .expect("first finalized turn should persist");
                    event_tx
                        .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                            locator: request.locator.clone(),
                            turn,
                        }))
                        .await
                        .expect("first finalized turn should be sent");
                    event_tx
                        .send(AgentWorkerEvent::RequestCompleted(
                            openjarvis::agent::CompletedAgentRequest {
                                locator: request.locator.clone(),
                                completed_at: Utc::now(),
                            },
                        ))
                        .await
                        .expect("first request should complete");
                }
                1 => {
                    let mut thread_context = request
                        .sessions
                        .lock_thread_context(&request.locator, request.incoming.received_at)
                        .await
                        .expect("second mock turn should lock");
                    thread_context
                        .begin_turn(
                            request.incoming.external_message_id.clone(),
                            request.incoming.received_at,
                        )
                        .expect("second mock turn should start");
                    let mut reply_to_source = true;
                    thread_context
                        .push_message(ChatMessage::new(
                            ChatMessageRole::User,
                            request.incoming.content.clone(),
                            request.incoming.received_at,
                        ))
                        .await
                        .expect("second mock turn user message should commit");
                    let tool_call_message =
                        ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now())
                            .with_tool_calls(vec![ChatToolCall {
                                id: "call_mock_1".to_string(),
                                name: "read".to_string(),
                                arguments: json!({ "path": "Cargo.toml" }),
                            }]);
                    thread_context
                        .push_message(tool_call_message.clone())
                        .await
                        .expect("tool-call message should commit");
                    let tool_call_event = TestCommittedEvent {
                        kind: AgentLoopEventKind::ToolCall,
                        content: "[openjarvis][tool_call] read {\"path\":\"Cargo.toml\"}"
                            .to_string(),
                        metadata: json!({
                            "tool": "read",
                            "arguments": { "path": "Cargo.toml" },
                            "tool_call_id": "call_mock_1",
                        }),
                    };
                    send_committed_message(
                        &event_tx,
                        &request,
                        build_dispatch_batch(&request, &mut reply_to_source, &[tool_call_event]),
                        tool_call_message.created_at,
                    )
                    .await;
                    let tool_result_message =
                        ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now())
                            .with_tool_call_id("call_mock_1");
                    thread_context
                        .push_message(tool_result_message.clone())
                        .await
                        .expect("tool result should commit");
                    let tool_result_event = TestCommittedEvent {
                        kind: AgentLoopEventKind::ToolResult,
                        content: "[openjarvis][tool_result] ok".to_string(),
                        metadata: json!({
                            "tool": "read",
                            "is_error": false,
                            "metadata": {},
                            "tool_call_id": "call_mock_1",
                        }),
                    };
                    send_committed_message(
                        &event_tx,
                        &request,
                        build_dispatch_batch(&request, &mut reply_to_source, &[tool_result_event]),
                        tool_result_message.created_at,
                    )
                    .await;
                    let assistant_message =
                        ChatMessage::new(ChatMessageRole::Assistant, "reply-second", Utc::now());
                    thread_context
                        .push_message(assistant_message.clone())
                        .await
                        .expect("final reply should commit");
                    let assistant_event = TestCommittedEvent {
                        kind: AgentLoopEventKind::TextOutput,
                        content: "reply-second".to_string(),
                        metadata: json!({
                            "source": "mock_agent",
                            "is_final": true,
                        }),
                    };
                    send_committed_message(
                        &event_tx,
                        &request,
                        build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
                        assistant_message.created_at,
                    )
                    .await;
                    let turn = thread_context
                        .finalize_turn_success("reply-second", Utc::now())
                        .expect("second mock turn should finalize");
                    request
                        .sessions
                        .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
                        .await
                        .expect("second finalized turn should persist");
                    event_tx
                        .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                            locator: request.locator.clone(),
                            turn,
                        }))
                        .await
                        .expect("second finalized turn should be sent");
                    event_tx
                        .send(AgentWorkerEvent::RequestCompleted(
                            openjarvis::agent::CompletedAgentRequest {
                                locator: request.locator.clone(),
                                completed_at: Utc::now(),
                            },
                        ))
                        .await
                        .expect("second request should complete");
                }
                _ => unreachable!("mock agent only scripts two requests"),
            }
        }
    })
}

fn spawn_truncation_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<ObservedAgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for step in 0..2 {
            let request = request_rx
                .recv()
                .await
                .expect("truncation mock agent should receive scripted request");
            let thread_context = request
                .sessions
                .load_thread_context(&request.locator)
                .await
                .expect("truncation request thread should load")
                .unwrap_or_else(|| {
                    Thread::new(
                        ThreadContextLocator::from(&request.locator),
                        request.incoming.received_at,
                    )
                });
            observed_requests.lock().await.push(ObservedAgentRequest {
                request: request.clone(),
                thread_context,
            });

            match step {
                0 => {
                    let mut thread_context = request
                        .sessions
                        .lock_thread_context(&request.locator, request.incoming.received_at)
                        .await
                        .expect("truncation turn should lock");
                    thread_context
                        .begin_turn(
                            request.incoming.external_message_id.clone(),
                            request.incoming.received_at,
                        )
                        .expect("truncation turn should start");
                    let mut reply_to_source = true;
                    thread_context
                        .push_message(ChatMessage::new(
                            ChatMessageRole::User,
                            request.incoming.content.clone(),
                            request.incoming.received_at,
                        ))
                        .await
                        .expect("truncation turn user message should commit");
                    for index in 1..=6 {
                        let content = format!("message_{index}");
                        let assistant_message = ChatMessage::new(
                            ChatMessageRole::Assistant,
                            content.clone(),
                            Utc::now(),
                        );
                        thread_context
                            .push_message(assistant_message.clone())
                            .await
                            .expect("truncation message should commit");
                        let assistant_event = TestCommittedEvent {
                            kind: AgentLoopEventKind::TextOutput,
                            content,
                            metadata: json!({
                                "source": "truncation_mock_agent",
                                "message_index": index,
                            }),
                        };
                        send_committed_message(
                            &event_tx,
                            &request,
                            build_dispatch_batch(
                                &request,
                                &mut reply_to_source,
                                &[assistant_event],
                            ),
                            assistant_message.created_at,
                        )
                        .await;
                    }
                    let turn = thread_context
                        .finalize_turn_success("message_6", Utc::now())
                        .expect("truncation turn should finalize");
                    request
                        .sessions
                        .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
                        .await
                        .expect("truncation finalized turn should persist");
                    event_tx
                        .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                            locator: request.locator.clone(),
                            turn,
                        }))
                        .await
                        .expect("truncation finalized turn should be sent");
                    event_tx
                        .send(AgentWorkerEvent::RequestCompleted(
                            openjarvis::agent::CompletedAgentRequest {
                                locator: request.locator.clone(),
                                completed_at: Utc::now(),
                            },
                        ))
                        .await
                        .expect("truncation request should complete");
                }
                1 => {
                    let mut thread_context = request
                        .sessions
                        .lock_thread_context(&request.locator, request.incoming.received_at)
                        .await
                        .expect("final truncation turn should lock");
                    thread_context
                        .begin_turn(
                            request.incoming.external_message_id.clone(),
                            request.incoming.received_at,
                        )
                        .expect("final truncation turn should start");
                    let mut reply_to_source = true;
                    thread_context
                        .push_message(ChatMessage::new(
                            ChatMessageRole::User,
                            request.incoming.content.clone(),
                            request.incoming.received_at,
                        ))
                        .await
                        .expect("final truncation turn user message should commit");
                    let assistant_message = ChatMessage::new(
                        ChatMessageRole::Assistant,
                        "final-after-truncation",
                        Utc::now(),
                    );
                    thread_context
                        .push_message(assistant_message.clone())
                        .await
                        .expect("final truncation reply should commit");
                    let assistant_event = TestCommittedEvent {
                        kind: AgentLoopEventKind::TextOutput,
                        content: "final-after-truncation".to_string(),
                        metadata: json!({
                            "source": "truncation_mock_agent",
                            "is_final": true,
                        }),
                    };
                    send_committed_message(
                        &event_tx,
                        &request,
                        build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
                        assistant_message.created_at,
                    )
                    .await;
                    let turn = thread_context
                        .finalize_turn_success("final-after-truncation", Utc::now())
                        .expect("final truncation turn should finalize");
                    request
                        .sessions
                        .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
                        .await
                        .expect("final truncation turn should persist");
                    event_tx
                        .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                            locator: request.locator.clone(),
                            turn,
                        }))
                        .await
                        .expect("final truncation finalized turn should be sent");
                    event_tx
                        .send(AgentWorkerEvent::RequestCompleted(
                            openjarvis::agent::CompletedAgentRequest {
                                locator: request.locator.clone(),
                                completed_at: Utc::now(),
                            },
                        ))
                        .await
                        .expect("final truncation request should complete");
                }
                _ => unreachable!("truncation mock agent only scripts two requests"),
            }
        }
    })
}

fn spawn_single_turn_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let request = request_rx
            .recv()
            .await
            .expect("single-commit mock agent should receive one request");
        observed_requests.lock().await.push(request.clone());
        let mut thread_context = request
            .sessions
            .lock_thread_context(&request.locator, request.incoming.received_at)
            .await
            .expect("single-turn mock should lock");
        thread_context
            .begin_turn(
                request.incoming.external_message_id.clone(),
                request.incoming.received_at,
            )
            .expect("single-turn mock should start");
        let mut reply_to_source = true;
        thread_context
            .push_message(ChatMessage::new(
                ChatMessageRole::User,
                request.incoming.content.clone(),
                request.incoming.received_at,
            ))
            .await
            .expect("single-turn mock user message should commit");
        let assistant_message =
            ChatMessage::new(ChatMessageRole::Assistant, "reply-single", Utc::now());
        thread_context
            .push_message(assistant_message.clone())
            .await
            .expect("single-turn reply should commit");
        let assistant_event = TestCommittedEvent {
            kind: AgentLoopEventKind::TextOutput,
            content: "reply-single".to_string(),
            metadata: json!({
                "source": "single_turn_mock_agent",
                "is_final": true,
            }),
        };
        send_committed_message(
            &event_tx,
            &request,
            build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
            assistant_message.created_at,
        )
        .await;
        let turn = thread_context
            .finalize_turn_success("reply-single", Utc::now())
            .expect("single-turn mock should finalize");
        request
            .sessions
            .commit_finalized_turn_locked(&request.locator, &mut thread_context, &turn)
            .await
            .expect("single-turn finalized turn should persist");
        event_tx
            .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                locator: request.locator.clone(),
                turn,
            }))
            .await
            .expect("single-turn finalized turn should be sent");
        event_tx
            .send(AgentWorkerEvent::RequestCompleted(
                openjarvis::agent::CompletedAgentRequest {
                    locator: request.locator.clone(),
                    completed_at: Utc::now(),
                },
            ))
            .await
            .expect("single-turn request should complete");
    })
}

fn spawn_compact_mock_agent_loop(
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let request = request_rx
            .recv()
            .await
            .expect("compact mock agent should receive one request");
        let mut compacted_thread = request
            .sessions
            .lock_thread_context(&request.locator, request.incoming.received_at)
            .await
            .expect("compact mock turn should lock");
        compacted_thread
            .begin_turn(
                request.incoming.external_message_id.clone(),
                request.incoming.received_at,
            )
            .expect("compact mock turn should start");
        let mut reply_to_source = true;
        compacted_thread
            .push_message(ChatMessage::new(
                ChatMessageRole::User,
                request.incoming.content.clone(),
                request.incoming.received_at,
            ))
            .await
            .expect("compact mock turn user message should commit");
        compacted_thread
            .replace_non_system_messages_after_compaction(vec![
                ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", Utc::now()),
                ChatMessage::new(ChatMessageRole::User, "继续", Utc::now()),
            ])
            .expect("compact mock should replace active view");
        let assistant_message = ChatMessage::new(
            ChatMessageRole::Assistant,
            "reply-after-compact",
            Utc::now(),
        );
        compacted_thread
            .push_message(assistant_message.clone())
            .await
            .expect("compact reply should commit");
        let assistant_event = TestCommittedEvent {
            kind: AgentLoopEventKind::TextOutput,
            content: "reply-after-compact".to_string(),
            metadata: json!({
                "source": "compact_mock_agent",
                "is_final": true,
            }),
        };
        send_committed_message(
            &event_tx,
            &request,
            build_dispatch_batch(&request, &mut reply_to_source, &[assistant_event]),
            assistant_message.created_at,
        )
        .await;
        let turn = compacted_thread
            .finalize_turn_success("reply-after-compact", Utc::now())
            .expect("compact mock turn should finalize");
        request
            .sessions
            .commit_finalized_turn_locked(&request.locator, &mut compacted_thread, &turn)
            .await
            .expect("compact mock turn should persist");

        event_tx
            .send(AgentWorkerEvent::TurnFinalized(FinalizedAgentTurn {
                locator: request.locator.clone(),
                turn,
            }))
            .await
            .expect("compact finalized turn should be sent");
        event_tx
            .send(AgentWorkerEvent::RequestCompleted(
                openjarvis::agent::CompletedAgentRequest {
                    locator: request.locator.clone(),
                    completed_at: Utc::now(),
                },
            ))
            .await
            .expect("compact request should complete");
    })
}

fn build_dispatch_event(
    request: &AgentRequest,
    kind: AgentLoopEventKind,
    content: &str,
    metadata: serde_json::Value,
    reply_to_source: bool,
) -> AgentDispatchEvent {
    AgentDispatchEvent {
        kind,
        content: content.to_string(),
        metadata,
        channel: request.incoming.channel.clone(),
        external_thread_id: request.incoming.external_thread_id.clone(),
        source_message_id: request.incoming.external_message_id.clone(),
        target: request.incoming.reply_target.clone(),
        session_id: request.locator.session_id.to_string(),
        session_channel: request.locator.channel.clone(),
        session_user_id: request.locator.user_id.clone(),
        session_external_thread_id: request.locator.external_thread_id.clone(),
        session_thread_id: request.locator.thread_id.to_string(),
        reply_to_source,
    }
}

fn build_dispatch_batch(
    request: &AgentRequest,
    reply_to_source: &mut bool,
    events: &[TestCommittedEvent],
) -> Vec<AgentDispatchEvent> {
    let built = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            build_dispatch_event(
                request,
                event.kind.clone(),
                &event.content,
                event.metadata.clone(),
                *reply_to_source && index == 0,
            )
        })
        .collect::<Vec<_>>();
    if !built.is_empty() {
        *reply_to_source = false;
    }
    built
}

async fn send_committed_message(
    event_tx: &mpsc::Sender<AgentWorkerEvent>,
    request: &AgentRequest,
    dispatch_events: Vec<AgentDispatchEvent>,
    committed_at: chrono::DateTime<Utc>,
) {
    for dispatch_event in dispatch_events {
        event_tx
            .send(AgentWorkerEvent::DispatchItemCommitted(
                CommittedAgentDispatchItem {
                    locator: request.locator.clone(),
                    dispatch_event,
                    committed_at,
                },
            ))
            .await
            .expect("committed message should be sent");
    }
}

fn openjarvis_bin() -> String {
    env!("CARGO_BIN_EXE_openjarvis").to_string()
}

fn text_response(content: &str) -> LLMResponse {
    LLMResponse {
        message: Some(ChatMessage::new(
            ChatMessageRole::Assistant,
            content,
            Utc::now(),
        )),
        tool_calls: Vec::new(),
    }
}

fn tool_only_response(name: &str, arguments: serde_json::Value) -> LLMResponse {
    LLMResponse {
        message: None,
        tool_calls: vec![ChatToolCall {
            id: "call_builtin_mcp".to_string(),
            name: name.to_string(),
            arguments,
        }],
    }
}

fn build_incoming() -> IncomingMessage {
    build_incoming_with("msg_1", "ou_xxx", None, "hello")
}

fn build_incoming_with(
    message_id: &str,
    user_id: &str,
    external_thread_id: Option<&str>,
    content: &str,
) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: user_id.to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: external_thread_id.map(|value| value.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn context_estimator() -> ContextBudgetEstimator {
    let config = AppConfig::default();
    ContextBudgetEstimator::from_config(config.llm_config(), config.agent_config().compact_config())
}

fn expected_context_summary(thread_context: &Thread) -> String {
    let estimator = context_estimator();
    let messages = thread_context.messages();
    let report = estimator.estimate(&messages, &[]);

    format!(
        "thread=`{external_thread_id}`\npersisted_messages={message_count}\ntotal_estimated_tokens={total_estimated_tokens}/{context_window_tokens} ({utilization_percent:.2}%)\nsystem_tokens={system_tokens}, chat_tokens={chat_tokens}, visible_tool_tokens={visible_tool_tokens}, reserved_output_tokens={reserved_output_tokens}",
        external_thread_id = thread_context.locator.external_thread_id,
        message_count = messages.len(),
        total_estimated_tokens = report.total_estimated_tokens,
        context_window_tokens = report.context_window_tokens,
        utilization_percent = report.utilization_ratio * 100.0,
        system_tokens = report.system_tokens(),
        chat_tokens = report.chat_tokens(),
        visible_tool_tokens = report.visible_tool_tokens(),
        reserved_output_tokens = report.reserved_output_tokens(),
    )
}
