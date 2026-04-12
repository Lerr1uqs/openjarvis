use chrono::Utc;
use openjarvis::{
    agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    model::{IncomingMessage, ReplyTarget},
    session::{MemorySessionStore, SessionManager, SessionStore},
    thread::ThreadRuntime,
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

fn build_thread_runtime(system_prompt: &str) -> Arc<ThreadRuntime> {
    let tool_registry = Arc::new(ToolRegistry::new());
    let memory_repository = Arc::new(MemoryRepository::new("."));
    let rebuilder = Arc::new(FeaturePromptRebuilder::new(
        Arc::clone(&tool_registry),
        Default::default(),
        system_prompt,
    ));
    Arc::new(ThreadRuntime::new(
        tool_registry,
        memory_repository,
        rebuilder,
        false,
    ))
}

#[tokio::test]
async fn session_manager_initializes_thread_before_returning_handle() {
    // 测试场景: SessionManager 首次派生线程时要先完成初始化消息落盘，再返回 thread handle。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager = SessionManager::with_store(store)
        .await
        .expect("manager should use memory store");
    manager.install_thread_runtime(build_thread_runtime("system prompt"));
    let incoming = build_incoming("hello");

    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let thread = manager
        .load_thread_context(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");

    assert!(thread.is_initialized());
    assert_eq!(thread.messages()[0].content, "system prompt");
}

#[tokio::test]
async fn session_manager_restores_thread_from_shared_store_without_session_metadata() {
    // 测试场景: thread 恢复只依赖稳定 thread identity，不依赖持久化 session 聚合。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager_a = SessionManager::with_store(Arc::clone(&store))
        .await
        .expect("manager a should use memory store");
    manager_a.install_thread_runtime(build_thread_runtime("system prompt"));
    let incoming = build_incoming("hello");
    let locator = manager_a
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    {
        let mut thread = manager_a
            .lock_thread_context(&locator, incoming.received_at)
            .await
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
    manager_b.install_thread_runtime(build_thread_runtime("system prompt"));
    let restored = manager_b
        .load_thread_context(&locator)
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
