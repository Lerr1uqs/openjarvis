use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use clap::Parser;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentLoopEventKind, AgentRequest, AgentRuntime, AgentWorker,
        AgentWorkerEvent, AgentWorkerHandle, CompletedAgentTurn, FailedAgentTurn,
    },
    channels::{Channel, ChannelRegistration},
    cli::OpenJarvisCli,
    config::{AppConfig, BUILTIN_MCP_SERVER_NAME, DEFAULT_ASSISTANT_SYSTEM_PROMPT},
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    router::ChannelRouter,
    router::ChannelRouterBuilder,
    session::{SessionKey, SessionManager, SessionStrategy},
    thread::ConversationThread,
};
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
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

struct SequenceProvider {
    responses: Arc<Mutex<Vec<LLMResponse>>>,
}

#[async_trait]
impl LLMProvider for SequenceProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        Ok(self.responses.lock().await.remove(0))
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
    let history = router.sessions().load_turn(&locator).await;
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

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.external_thread_id, "thread_shared");
    assert_eq!(history.len(), 6);
    assert_eq!(history[0].content, "first question");
    assert_eq!(history[1].content, "reply-first");
    assert_eq!(history[5].content, "reply-second");
    assert_eq!(thread.turns[0].messages.len(), 2);
    assert_eq!(thread.turns[1].messages.len(), 4);
    assert_eq!(thread.turns[0].messages[0].content, "first question");
    assert_eq!(thread.turns[0].messages[1].content, "reply-first");
    assert_eq!(thread.turns[1].messages[0].content, "second question");
    assert_eq!(thread.turns[1].messages[1].tool_calls[0].id, "call_mock_1");
    assert_eq!(
        thread.turns[1].messages[2].tool_call_id.as_deref(),
        Some("call_mock_1")
    );
    assert_eq!(thread.turns[1].messages[3].content, "reply-second");

    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].content, "reply-first");
    assert!(recorded[1].content.contains("[openjarvis][tool_call]"));
    assert!(recorded[2].content.contains("[openjarvis][tool_result]"));
    assert_eq!(recorded[3].content, "reply-second");
}

#[tokio::test]
async fn router_applies_five_message_truncation_strategy_before_next_turn() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let agent_harness = build_truncation_mock_agent_handle(Arc::clone(&observed_requests));
    let event_keepalive_tx = agent_harness.event_keepalive_tx; // test-only: prevents the mock downstream channel from looking crashed.
    let sessions = SessionManager::with_strategy(SessionStrategy {
        max_messages_per_thread: 5,
    });
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
    let locator = observed_requests[0].locator.clone();
    let history = router.sessions().load_turn(&locator).await;
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
    assert!(observed_requests[0].history.is_empty());
    assert_eq!(observed_requests[1].history.len(), 5);
    assert_eq!(
        observed_requests[1]
            .history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "message_2".to_string(),
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.external_thread_id, "thread_truncation");
    assert_eq!(thread.turns[0].messages.len(), 3);
    assert_eq!(thread.turns[1].messages.len(), 2);
    assert_eq!(
        thread.turns[0]
            .messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );
    assert_eq!(
        thread.turns[1]
            .messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "check history after truncation".to_string(),
            "final-after-truncation".to_string(),
        ]
    );
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].content, "message_4");
    assert_eq!(history[2].content, "message_6");
    assert_eq!(history[3].content, "check history after truncation");
    assert_eq!(history[4].content, "final-after-truncation");

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
        .await;

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][echo][SUCCESS]: keep everything"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "echo");
    assert_eq!(recorded[0].metadata["command_status"], "SUCCESS");
    assert!(request_rx.try_recv().is_err());
    assert!(session.is_none());
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
        .await;

    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].content,
        "[Command][missing][FAILED]: unknown command"
    );
    assert_eq!(recorded[0].metadata["event_kind"], "Command");
    assert_eq!(recorded[0].metadata["command_name"], "missing");
    assert_eq!(recorded[0].metadata["command_status"], "FAILED");
    assert!(request_rx.try_recv().is_err());
    assert!(session.is_none());
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
    let locator = router.sessions().load_or_create_thread(&incoming).await;

    let send_task = tokio::spawn(async move {
        event_tx
            .send(AgentWorkerEvent::TurnFailed(FailedAgentTurn {
                active_thread: empty_thread(locator.thread_id, &locator.external_thread_id),
                locator,
                incoming,
                error: "failed to call llm provider `openai_compatible` model `demo-model` at `https://provider.test/v1`: provider said 429: rate limit exceeded".to_string(),
                loaded_toolsets: Vec::new(),
                completed_at: Utc::now(),
            }))
            .await
            .expect("failed turn should be sent");
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
    assert_eq!(recorded[0].metadata["event_kind"], "AgentError");
    assert!(
        recorded[0].metadata["error"]
            .as_str()
            .expect("router should preserve failure metadata")
            .contains("provider said 429: rate limit exceeded")
    );
    assert_eq!(history.threads.len(), 1);
    let thread = history
        .threads
        .values()
        .next()
        .expect("thread should exist after failed turn");
    assert_eq!(thread.turns.len(), 1);
    assert!(
        thread.turns[0].messages[1]
            .content
            .contains("provider said 429: rate limit exceeded")
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
    let history = router.sessions().load_turn(&locator).await;
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
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].messages.len(), 2);
    assert_eq!(
        thread.turns[0]
            .messages
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
    let history = thread.load_messages();

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.turns[0].messages[0].content, "这是压缩后的上下文");
    assert_eq!(thread.turns[0].messages[1].content, "继续");
    assert_eq!(thread.turns[1].messages[0].content, "reply-after-compact");
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
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
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
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "reply-first",
                            json!({
                                "source": "mock_agent",
                                "is_final": true,
                            }),
                            true,
                        )))
                        .await
                        .expect("first dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            active_thread: request.thread.clone(),
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![ChatMessage::new(
                                ChatMessageRole::Assistant,
                                "reply-first",
                                Utc::now(),
                            )],
                            prepend_incoming_user: true,
                            loaded_toolsets: Vec::new(),
                            tool_events: Vec::new(),
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("first completed turn should be sent");
                }
                1 => {
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::ToolCall,
                            "[openjarvis][tool_call] read {\"path\":\"Cargo.toml\"}",
                            json!({
                                "tool": "read",
                                "arguments": { "path": "Cargo.toml" },
                                "tool_call_id": "call_mock_1",
                            }),
                            true,
                        )))
                        .await
                        .expect("tool_call dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::ToolResult,
                            "[openjarvis][tool_result] ok",
                            json!({
                                "tool": "read",
                                "is_error": false,
                                "metadata": {},
                                "tool_call_id": "call_mock_1",
                            }),
                            false,
                        )))
                        .await
                        .expect("tool_result dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "reply-second",
                            json!({
                                "source": "mock_agent",
                                "is_final": true,
                            }),
                            false,
                        )))
                        .await
                        .expect("final text dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            active_thread: request.thread.clone(),
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![
                                ChatMessage::new(ChatMessageRole::Assistant, "", Utc::now())
                                    .with_tool_calls(vec![ChatToolCall {
                                        id: "call_mock_1".to_string(),
                                        name: "read".to_string(),
                                        arguments: json!({ "path": "Cargo.toml" }),
                                    }]),
                                ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now())
                                    .with_tool_call_id("call_mock_1"),
                                ChatMessage::new(
                                    ChatMessageRole::Assistant,
                                    "reply-second",
                                    Utc::now(),
                                ),
                            ],
                            prepend_incoming_user: true,
                            loaded_toolsets: Vec::new(),
                            tool_events: Vec::new(),
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("second completed turn should be sent");
                }
                _ => unreachable!("mock agent only scripts two requests"),
            }
        }
    })
}

fn spawn_truncation_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for step in 0..2 {
            let request = request_rx
                .recv()
                .await
                .expect("truncation mock agent should receive scripted request");
            observed_requests.lock().await.push(request.clone());

            match step {
                0 => {
                    let mut turn_messages = Vec::new();
                    for index in 1..=6 {
                        let content = format!("message_{index}");
                        event_tx
                            .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                                &request,
                                AgentLoopEventKind::TextOutput,
                                &content,
                                json!({
                                    "source": "truncation_mock_agent",
                                    "message_index": index,
                                }),
                                index == 1,
                            )))
                            .await
                            .expect("truncation dispatch should be sent");
                        turn_messages.push(ChatMessage::new(
                            ChatMessageRole::Assistant,
                            content,
                            Utc::now(),
                        ));
                    }
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            active_thread: request.thread.clone(),
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: turn_messages,
                            prepend_incoming_user: true,
                            loaded_toolsets: Vec::new(),
                            tool_events: Vec::new(),
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("truncation completed turn should be sent");
                }
                1 => {
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "final-after-truncation",
                            json!({
                                "source": "truncation_mock_agent",
                                "is_final": true,
                            }),
                            true,
                        )))
                        .await
                        .expect("final truncation dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            active_thread: request.thread.clone(),
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![ChatMessage::new(
                                ChatMessageRole::Assistant,
                                "final-after-truncation",
                                Utc::now(),
                            )],
                            prepend_incoming_user: true,
                            loaded_toolsets: Vec::new(),
                            tool_events: Vec::new(),
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("final truncation completed turn should be sent");
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
            .expect("single-turn mock agent should receive one request");
        observed_requests.lock().await.push(request.clone());

        event_tx
            .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                &request,
                AgentLoopEventKind::TextOutput,
                "reply-single",
                json!({
                    "source": "single_turn_mock_agent",
                    "is_final": true,
                }),
                true,
            )))
            .await
            .expect("single-turn dispatch should be sent");
        event_tx
            .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                active_thread: request.thread.clone(),
                locator: request.locator,
                incoming: request.incoming,
                messages: vec![ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "reply-single",
                    Utc::now(),
                )],
                prepend_incoming_user: true,
                loaded_toolsets: Vec::new(),
                tool_events: Vec::new(),
                completed_at: Utc::now(),
            }))
            .await
            .expect("single-turn completed turn should be sent");
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
        let mut active_thread = request.thread.clone();
        active_thread.turns = vec![openjarvis::thread::ConversationTurn::new(
            None,
            vec![
                ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", Utc::now()),
                ChatMessage::new(ChatMessageRole::User, "继续", Utc::now()),
            ],
            Utc::now(),
            Utc::now(),
        )];

        event_tx
            .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                &request,
                AgentLoopEventKind::TextOutput,
                "reply-after-compact",
                json!({
                    "source": "compact_mock_agent",
                    "is_final": true,
                }),
                true,
            )))
            .await
            .expect("compact dispatch should be sent");
        event_tx
            .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                active_thread,
                locator: request.locator,
                incoming: request.incoming,
                messages: vec![ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "reply-after-compact",
                    Utc::now(),
                )],
                prepend_incoming_user: false,
                loaded_toolsets: Vec::new(),
                tool_events: Vec::new(),
                completed_at: Utc::now(),
            }))
            .await
            .expect("compact completed turn should be sent");
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
        thread_id: request.incoming.thread_id.clone(),
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
    thread_id: Option<&str>,
    content: &str,
) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: user_id.to_string(),
        user_name: None,
        content: content.to_string(),
        thread_id: thread_id.map(|value| value.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn empty_thread(thread_id: Uuid, external_thread_id: &str) -> ConversationThread {
    ConversationThread::with_id(thread_id, external_thread_id.to_string(), Utc::now())
}
