use chrono::Utc;
use openjarvis::{
    agent::{MemoryRepository, ToolRegistry},
    config::AgentCompactConfig,
    model::{IncomingMessage, ReplyTarget},
    session::{MemorySessionStore, SessionManager, SessionStore},
    thread::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, ThreadAgentKind, ThreadRuntime},
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_session".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_session".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_session".to_string()),
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
