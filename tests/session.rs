use chrono::Utc;
use openjarvis::{
    agent::{MemoryRepository, ToolRegistry},
    config::AgentCompactConfig,
    model::{IncomingMessage, ReplyTarget},
    session::{MemorySessionStore, SessionManager, SessionStore, ThreadLocator},
    thread::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, SubagentSpawnMode, ThreadAgentKind, ThreadRuntime},
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn build_incoming(content: &str) -> IncomingMessage {
    build_incoming_for_thread(content, "chat_session")
}

fn build_incoming_for_thread(content: &str, external_thread_id: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_session".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_session".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some(external_thread_id.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_session".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread_runtime() -> Arc<ThreadRuntime> {
    let tool_registry = Arc::new(ToolRegistry::new());
    let memory_repository = Arc::new(MemoryRepository::new("."));
    Arc::new(ThreadRuntime::new(
        tool_registry,
        memory_repository,
        AgentCompactConfig::default(),
    ))
}

#[tokio::test]
async fn session_manager_initializes_thread_before_returning_handle() {
    // 测试场景: SessionManager 首次派生线程时要先完成初始化消息落盘，再返回 thread handle。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("hello");

    let locator = manager
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    let thread = manager
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");

    assert!(thread.is_initialized());
    assert_eq!(
        thread.messages()[0].content,
        DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim()
    );
}

#[tokio::test]
async fn session_manager_restores_thread_from_shared_store_without_session_metadata() {
    // 测试场景: thread 恢复只依赖稳定 thread identity，不依赖持久化 session 聚合。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager_a = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("manager a should use memory store");
    manager_a.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("hello");
    let locator = manager_a
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    {
        let mut thread = manager_a
            .lock_thread(&locator, incoming.received_at)
            .await
            .expect("thread lock result should resolve")
            .expect("thread should lock");
        thread
            .push_message(openjarvis::context::ChatMessage::new(
                openjarvis::context::ChatMessageRole::User,
                "persisted user message",
                incoming.received_at,
            ))
            .await
            .expect("message should persist");
    }

    let manager_b = SessionManager::with_store(store)
        .await
        .expect("manager b should use shared store");
    manager_b.install_thread_runtime(build_thread_runtime());
    let restored = manager_b
        .load_thread(&locator)
        .await
        .expect("thread should restore")
        .expect("thread should exist");

    assert_eq!(restored.locator.thread_id, locator.thread_id.to_string());
    assert_eq!(
        restored
            .messages()
            .iter()
            .find(|message| message.role == openjarvis::context::ChatMessageRole::User)
            .map(|message| message.content.clone()),
        Some("persisted user message".to_string())
    );
}

#[tokio::test]
async fn session_manager_browser_agent_preloads_browser_toolset() {
    // 测试场景: Browser agent 线程在 create 路径中要写入浏览器角色前缀，并预绑定 browser toolset。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("open browser");

    let locator = manager
        .create_thread(&incoming, ThreadAgentKind::Browser)
        .await
        .expect("browser thread should resolve");
    let thread = manager
        .load_thread(&locator)
        .await
        .expect("browser thread should load")
        .expect("browser thread should exist");

    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Browser);
    assert!(
        thread
            .load_toolsets()
            .iter()
            .any(|toolset| toolset == "browser")
    );
    assert!(thread.messages()[0].content.contains("浏览器线程代理"));
}

#[tokio::test]
async fn session_manager_repeated_create_keeps_persisted_thread_agent() {
    // 测试场景: 已初始化线程再次走 create 路径时，必须沿用已持久化的 thread agent 真相，不改写稳定前缀。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("hello");

    let locator = manager
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("main thread should resolve");
    manager
        .create_thread(&incoming, ThreadAgentKind::Browser)
        .await
        .expect("repeated create should still resolve");

    let thread = manager
        .load_thread(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    assert_eq!(thread.thread_agent_kind(), ThreadAgentKind::Main);
    assert_eq!(
        thread.messages()[0].content,
        DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim()
    );
}

#[tokio::test]
async fn session_manager_load_and_lock_return_none_for_missing_thread() {
    // 测试场景: 纯 load / lock 路径在 miss 时必须显式返回缺失，而不是偷偷创建新线程。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    let incoming = build_incoming("hello");
    let session_key = openjarvis::session::SessionKey::from_incoming(&incoming);
    let locator = openjarvis::session::ThreadLocator::new(
        session_key.derive_session_id(),
        &incoming,
        "missing_thread",
        session_key.derive_thread_id("missing_thread"),
    );

    assert!(
        manager
            .load_thread(&locator)
            .await
            .expect("load_thread should return a result")
            .is_none()
    );
    assert!(
        manager
            .lock_thread(&locator, incoming.received_at)
            .await
            .expect("lock_thread should return a result")
            .is_none()
    );
}

#[tokio::test]
async fn session_manager_child_thread_reuses_same_profile_across_spawn_modes() {
    // 测试场景: 同一父线程下同一 subagent profile 只允许一个 child thread，spawn_mode 只更新元数据不派生第二个实例。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("hello");
    let parent_locator = manager
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");

    let child_persist =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_yolo = ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Yolo);
    let first = manager
        .create_thread_at(
            &child_persist,
            incoming.received_at,
            ThreadAgentKind::Browser,
        )
        .await
        .expect("persist child should resolve");
    let second = manager
        .create_thread_at(&child_yolo, incoming.received_at, ThreadAgentKind::Browser)
        .await
        .expect("yolo child should reuse existing thread");

    assert_eq!(first.thread_id, second.thread_id);
    let child = manager
        .load_thread(&second)
        .await
        .expect("child thread should load")
        .expect("child thread should exist");
    assert_eq!(
        child
            .child_thread_identity()
            .map(|value| value.parent_thread_id.as_str()),
        Some(&parent_locator.thread_id.to_string()[..])
    );
    assert_eq!(
        child.child_thread_identity().map(|value| value.spawn_mode),
        Some(SubagentSpawnMode::Yolo)
    );
    assert_eq!(
        manager
            .list_child_threads(&parent_locator)
            .await
            .expect("child threads should list")
            .len(),
        1
    );
}

#[tokio::test]
async fn session_manager_isolates_same_subagent_key_across_different_parents() {
    // 测试场景: 不同父线程下同名 subagent 必须解析成不同 child thread id，避免状态串线。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime());

    let parent_a = manager
        .create_thread(
            &build_incoming_for_thread("hello-a", "chat_session_a"),
            ThreadAgentKind::Main,
        )
        .await
        .expect("parent a should resolve");
    let parent_b = manager
        .create_thread(
            &build_incoming_for_thread("hello-b", "chat_session_b"),
            ThreadAgentKind::Main,
        )
        .await
        .expect("parent b should resolve");

    let child_a = ThreadLocator::for_child(&parent_a, "browser", SubagentSpawnMode::Persist);
    let child_b = ThreadLocator::for_child(&parent_b, "browser", SubagentSpawnMode::Persist);

    assert_ne!(child_a.thread_id, child_b.thread_id);
}

#[tokio::test]
async fn session_manager_restores_and_removes_child_thread_records() {
    // 测试场景: child thread 落盘后应可恢复命中既有记录，remove 后 load/list 都必须显式 miss。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager_a = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("manager a should use memory store");
    manager_a.install_thread_runtime(build_thread_runtime());
    let incoming = build_incoming("restore child");
    let parent_locator = manager_a
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");
    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_locator = manager_a
        .create_thread_at(
            &child_locator,
            incoming.received_at,
            ThreadAgentKind::Browser,
        )
        .await
        .expect("child thread should resolve");

    {
        let mut child = manager_a
            .lock_thread(&child_locator, incoming.received_at)
            .await
            .expect("child lock result should resolve")
            .expect("child thread should lock");
        child
            .push_message(openjarvis::context::ChatMessage::new(
                openjarvis::context::ChatMessageRole::User,
                "persisted child message",
                incoming.received_at,
            ))
            .await
            .expect("child message should persist");
    }

    let manager_b = SessionManager::with_store(store)
        .await
        .expect("manager b should use shared store");
    manager_b.install_thread_runtime(build_thread_runtime());
    let restored = manager_b
        .create_thread_at(
            &child_locator,
            incoming.received_at,
            ThreadAgentKind::Browser,
        )
        .await
        .expect("child thread should restore");
    assert_eq!(restored.thread_id, child_locator.thread_id);
    assert!(
        manager_b
            .load_thread(&restored)
            .await
            .expect("restored child should load")
            .expect("restored child should exist")
            .messages()
            .iter()
            .any(|message| message.content == "persisted child message")
    );

    assert!(
        manager_b
            .remove_thread(&restored)
            .await
            .expect("child remove should succeed")
    );
    assert!(
        manager_b
            .load_thread(&restored)
            .await
            .expect("removed child load should resolve")
            .is_none()
    );
    assert!(
        manager_b
            .list_child_threads(&parent_locator)
            .await
            .expect("child list should resolve")
            .is_empty()
    );
}
