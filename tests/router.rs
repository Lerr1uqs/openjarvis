use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentWorkerHandle, FeatureResolver, MemoryRepository, ToolRegistry},
    channels::{Channel, ChannelRegistration},
    config::AppConfig,
    context::ChatMessageRole,
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    router::ChannelRouter,
    session::{SessionManager, ThreadLocator},
    thread::{
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT, Features, SubagentSpawnMode, ThreadAgentKind,
        ThreadRuntime,
    },
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};
use uuid::Uuid;

#[derive(Default)]
struct FakeFeishuChannelInner {
    registration: Mutex<Option<mpsc::Sender<IncomingMessage>>>,
    outgoing: Mutex<Vec<OutgoingMessage>>,
}

#[derive(Clone, Default)]
struct FakeFeishuChannel {
    inner: Arc<FakeFeishuChannelInner>,
}

#[async_trait]
impl Channel for FakeFeishuChannel {
    fn name(&self) -> &'static str {
        "feishu"
    }

    async fn start(self: Arc<Self>, registration: ChannelRegistration) -> Result<()> {
        *self.inner.registration.lock().await = Some(registration.incoming_tx);
        let channel = Arc::clone(&self);
        tokio::spawn(async move {
            let mut outgoing_rx = registration.outgoing_rx;
            while let Some(message) = outgoing_rx.recv().await {
                channel.inner.outgoing.lock().await.push(message);
            }
        });
        Ok(())
    }
}

impl FakeFeishuChannel {
    async fn send(&self, incoming: IncomingMessage) {
        self.inner
            .registration
            .lock()
            .await
            .as_ref()
            .expect("channel should be registered")
            .send(incoming)
            .await
            .expect("message should enter router");
    }

    async fn outgoing_messages(&self) -> Vec<OutgoingMessage> {
        self.inner.outgoing.lock().await.clone()
    }
}

fn build_incoming(message_id: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_router".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_router".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_router".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread_runtime() -> Arc<ThreadRuntime> {
    Arc::new(ThreadRuntime::with_feature_resolver(
        Arc::new(ToolRegistry::new()),
        Arc::new(MemoryRepository::new(".")),
        AppConfig::default().agent_config().compact_config().clone(),
        FeatureResolver::development_default(Features::default()),
    ))
}

#[tokio::test]
async fn router_feishu_deduper_blocks_duplicate_incoming_messages() {
    // 测试场景: Feishu 相同 external_message_id 在 TTL 内重复投递时只能进入一次主链路。
    let sessions = SessionManager::new();
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (_event_tx, event_rx) = mpsc::channel(8);
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        sessions,
    );
    let channel = FakeFeishuChannel::default();
    router
        .register_channel(Box::new(channel.clone()))
        .await
        .expect("fake feishu channel should register");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let router_task = tokio::spawn(async move {
        router
            .run_until_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let incoming = build_incoming("msg_dedup", "hello");
    channel.send(incoming.clone()).await;
    channel.send(incoming).await;

    let first = timeout(Duration::from_secs(5), request_rx.recv())
        .await
        .expect("first request should arrive")
        .expect("first request should exist");
    assert_eq!(
        first.incoming.external_message_id.as_deref(),
        Some("msg_dedup")
    );
    assert!(
        timeout(Duration::from_millis(300), request_rx.recv())
            .await
            .is_err(),
        "duplicate message should not enqueue a second request"
    );

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
}

#[tokio::test]
async fn router_executes_new_command_without_agent_dispatch() {
    // 测试场景: `/new` 通过命令路径直接重初始化当前线程，不进入 agent worker。
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(build_thread_runtime());
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (_event_tx, event_rx) = mpsc::channel(8);
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        sessions.clone(),
    );
    let channel = FakeFeishuChannel::default();
    router
        .register_channel(Box::new(channel.clone()))
        .await
        .expect("fake feishu channel should register");

    let seed = build_incoming("msg_seed", "hello");
    let locator = sessions
        .create_thread(&seed, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    {
        let mut thread = sessions
            .lock_thread(&locator, seed.received_at)
            .await
            .expect("thread lock result should resolve")
            .expect("thread should lock");
        thread
            .push_message(openjarvis::context::ChatMessage::new(
                openjarvis::context::ChatMessageRole::User,
                "persisted before new",
                seed.received_at,
            ))
            .await
            .expect("seed message should persist");
    }

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let router_task = tokio::spawn(async move {
        router
            .run_until_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    channel.send(build_incoming("msg_new", "/new")).await;

    let reply = timeout(Duration::from_secs(5), async {
        loop {
            let messages = channel.outgoing_messages().await;
            if let Some(message) = messages.last() {
                break message.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("command reply should arrive");
    assert!(reply.content.contains("[Command][new][SUCCESS]"));
    assert!(
        timeout(Duration::from_millis(300), request_rx.recv())
            .await
            .is_err(),
        "new command should not dispatch to agent worker"
    );
    let thread = sessions
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    assert!(thread.is_initialized());
    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Main);
    assert!(
        thread
            .messages()
            .iter()
            .all(|message| message.role == ChatMessageRole::System)
    );

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
}

#[tokio::test]
async fn router_new_command_reinitializes_attached_persist_child_threads() {
    // 测试场景: Router 真实命令入口执行 `/new` 时，必须把当前 parent 名下的 persist child thread 一起 reset/reinit。
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(build_thread_runtime());
    let (request_tx, mut request_rx) = mpsc::channel(8);
    let (_event_tx, event_rx) = mpsc::channel(8);
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        AgentWorkerHandle {
            request_tx,
            event_rx,
        },
        sessions.clone(),
    );
    let channel = FakeFeishuChannel::default();
    router
        .register_channel(Box::new(channel.clone()))
        .await
        .expect("fake feishu channel should register");

    let seed = build_incoming("msg_seed_with_child", "hello");
    let parent_locator = sessions
        .create_thread(&seed, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");
    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_locator = sessions
        .create_thread_at(&child_locator, seed.received_at, ThreadAgentKind::Browser)
        .await
        .expect("persist child thread should resolve");
    {
        let mut child_thread = sessions
            .lock_thread(&child_locator, seed.received_at)
            .await
            .expect("child lock result should resolve")
            .expect("child thread should lock");
        child_thread
            .push_message(openjarvis::context::ChatMessage::new(
                openjarvis::context::ChatMessageRole::User,
                "child history before /new",
                seed.received_at,
            ))
            .await
            .expect("child message should persist");
    }

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let router_task = tokio::spawn(async move {
        router
            .run_until_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    channel
        .send(build_incoming("msg_new_with_child", "/new"))
        .await;

    let reply = timeout(Duration::from_secs(5), async {
        loop {
            let messages = channel.outgoing_messages().await;
            if let Some(message) = messages.last() {
                break message.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("command reply should arrive");
    assert!(reply.content.contains("[Command][new][SUCCESS]"));
    assert!(
        timeout(Duration::from_millis(300), request_rx.recv())
            .await
            .is_err(),
        "new command should not dispatch to agent worker"
    );

    let child_thread = sessions
        .load_thread(&child_locator)
        .await
        .expect("child thread should load")
        .expect("child thread should exist");
    assert!(child_thread.is_initialized());
    assert_eq!(child_thread.thread_agent_kind(), ThreadAgentKind::Browser);
    assert_eq!(
        child_thread.messages()[0].content,
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim()
    );
    assert!(
        child_thread
            .messages()
            .iter()
            .all(|message| message.role == ChatMessageRole::System)
    );

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
}
