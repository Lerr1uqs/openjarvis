#[path = "support/mod.rs"]
mod support;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentWorker, AgentWorkerEvent, AgentWorkerHandle, FeatureResolver, MemoryRepository,
        ToolRegistry,
    },
    channels::{Channel, ChannelRegistration},
    config::AppConfig,
    context::ChatMessageRole,
    llm::{LLMProvider, LLMRequest, LLMResponse},
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    queue::TopicQueueRuntimeConfig,
    router::ChannelRouter,
    session::{SessionManager, ThreadLocator},
    thread::{
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT, Features, SubagentSpawnMode, ThreadAgentKind,
        ThreadRuntime,
    },
};
use serde_json::json;
use std::sync::Arc;
use support::TestTopicQueue;
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

fn noop_agent_handle() -> (mpsc::Sender<AgentWorkerEvent>, AgentWorkerHandle) {
    let (event_tx, event_rx) = mpsc::channel(8);
    (event_tx, AgentWorkerHandle::noop(event_rx))
}

async fn wait_for_condition<F, Fut>(check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    timeout(Duration::from_secs(5), async move {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("condition should be satisfied");
}

struct EchoLastUserProvider;

#[async_trait]
impl LLMProvider for EchoLastUserProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        let content = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ChatMessageRole::User)
            .map(|message| message.content.clone())
            .ok_or_else(|| anyhow!("missing user message"))?;
        Ok(LLMResponse {
            items: vec![openjarvis::context::ChatMessage::new(
                ChatMessageRole::Assistant,
                format!("echo:{content}"),
                Utc::now(),
            )],
        })
    }
}

#[tokio::test]
async fn router_feishu_deduper_blocks_duplicate_incoming_messages() {
    // 测试场景: Feishu 相同 external_message_id 在 TTL 内重复投递时只能进入一次 durable queue。
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::default());
    let (event_tx, handle) = noop_agent_handle();
    let mut router =
        ChannelRouter::with_session_manager_and_agent_handle(handle, queue.clone(), sessions);
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

    wait_for_condition(|| {
        let queue = queue.clone();
        async move { queue.snapshot_messages().await.len() == 1 }
    })
    .await;

    let messages = queue.snapshot_messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].payload.incoming.external_message_id.as_deref(),
        Some("msg_dedup")
    );

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
    drop(event_tx);
}

#[tokio::test]
async fn router_executes_new_command_without_queue_enqueue() {
    // 测试场景: `/new` 继续走 command 前置路径，不写 queue，也不触发普通消息 worker。
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(build_thread_runtime());
    let queue = Arc::new(TestTopicQueue::default());
    let (event_tx, handle) = noop_agent_handle();
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        handle,
        queue.clone(),
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
        queue.snapshot_messages().await.is_empty(),
        "command should not enter queue"
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
    drop(event_tx);
}

#[tokio::test]
async fn router_new_command_reinitializes_attached_persist_child_threads() {
    // 测试场景: Router 真实命令入口执行 `/new` 时，必须把当前 parent 名下的 persist child thread 一起 reset/reinit。
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(build_thread_runtime());
    let queue = Arc::new(TestTopicQueue::default());
    let (event_tx, handle) = noop_agent_handle();
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        handle,
        queue.clone(),
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
        queue.snapshot_messages().await.is_empty(),
        "command should not enter queue"
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
    drop(event_tx);
}

#[tokio::test]
async fn router_reuses_loaded_thread_identity_before_enqueue() {
    // 测试场景: 已加载 thread 再来普通消息时，router 必须直接复用 locator，不重复走 cold prepare。
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(build_thread_runtime());
    let queue = Arc::new(TestTopicQueue::default());
    let (event_tx, handle) = noop_agent_handle();
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(
        handle,
        queue.clone(),
        sessions.clone(),
    );
    let channel = FakeFeishuChannel::default();
    router
        .register_channel(Box::new(channel.clone()))
        .await
        .expect("fake feishu channel should register");

    let seed = build_incoming("msg_loaded_seed", "hello");
    let locator = sessions
        .create_thread(&seed, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let router_task = tokio::spawn(async move {
        router
            .run_until_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    channel
        .send(build_incoming("msg_loaded", "follow-up"))
        .await;

    wait_for_condition(|| {
        let queue = queue.clone();
        async move { queue.snapshot_messages().await.len() == 1 }
    })
    .await;

    let messages = queue.snapshot_messages().await;
    assert_eq!(messages[0].payload.locator.thread_id, locator.thread_id);
    assert_eq!(messages[0].topic, locator.thread_key());
    let thread = sessions
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    assert!(thread.is_initialized());

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
    drop(event_tx);
}

#[tokio::test]
async fn router_processes_same_thread_messages_in_order_and_worker_exits_idle() {
    // 测试场景: 同一 thread_key 的两条普通消息必须顺序处理，domain worker 在队列清空后还能空闲退出。
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::new(TopicQueueRuntimeConfig {
        lease_ttl: Duration::from_secs(2),
        heartbeat_interval: Duration::from_millis(200),
        idle_timeout: Duration::from_millis(300),
        reconcile_interval: Duration::from_millis(200),
        pending_topic_scan_limit: 32,
    }));
    let worker = AgentWorker::builder()
        .llm(Arc::new(EchoLastUserProvider))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut router = ChannelRouter::builder()
        .agent(worker)
        .topic_queue(queue.clone())
        .session_manager(sessions.clone())
        .build()
        .expect("router should build");
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

    channel.send(build_incoming("msg_seq_1", "first")).await;
    channel.send(build_incoming("msg_seq_2", "second")).await;

    wait_for_condition(|| {
        let channel = channel.clone();
        async move { channel.outgoing_messages().await.len() >= 2 }
    })
    .await;

    let locator = sessions.resolve_locator(&build_incoming("msg_tmp", "first"));
    let thread = sessions
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    let non_system = thread
        .messages()
        .into_iter()
        .filter(|message| message.role != ChatMessageRole::System)
        .collect::<Vec<_>>();
    assert_eq!(non_system[0].role, ChatMessageRole::User);
    assert_eq!(non_system[0].content, "first");
    assert_eq!(non_system[1].role, ChatMessageRole::Assistant);
    assert_eq!(non_system[1].content, "echo:first");
    assert_eq!(non_system[2].role, ChatMessageRole::User);
    assert_eq!(non_system[2].content, "second");
    assert_eq!(non_system[3].role, ChatMessageRole::Assistant);
    assert_eq!(non_system[3].content, "echo:second");

    wait_for_condition(|| {
        let queue = queue.clone();
        async move { queue.active_worker_domains(Utc::now()).await.is_empty() }
    })
    .await;

    let _ = shutdown_tx.send(());
    router_task
        .await
        .expect("router task should join")
        .expect("router should exit cleanly");
}
