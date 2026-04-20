use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    session::{MemorySessionStore, SessionStore, ThreadLocator},
    thread::{
        ChildThreadIdentity, DEFAULT_ASSISTANT_SYSTEM_PROMPT, DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT,
        Feature, SubagentSpawnMode, Thread, ThreadAgent, ThreadAgentKind, ThreadContextLocator,
        derive_child_thread_id, derive_internal_thread_id,
    },
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn build_incoming() -> openjarvis::model::IncomingMessage {
    openjarvis::model::IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_thread".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_thread".to_string(),
        user_name: None,
        content: "hello".to_string(),
        external_thread_id: Some("chat_thread".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: openjarvis::model::ReplyTarget {
            receive_id: "oc_thread".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_locator(incoming: &openjarvis::model::IncomingMessage) -> ThreadLocator {
    let session_key = openjarvis::session::SessionKey::from_incoming(incoming);
    ThreadLocator::new(
        session_key.derive_session_id(),
        incoming,
        incoming.resolved_external_thread_id(),
        session_key.derive_thread_id(&incoming.resolved_external_thread_id()),
    )
}

#[tokio::test]
async fn push_message_success_immediately_persists_snapshot() {
    // 测试场景: `push_message(...)` 成功返回即已把消息写入 store，不再依赖额外 finalize/commit。
    let incoming = build_incoming();
    let locator = build_locator(&incoming);
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    store
        .initialize_schema()
        .await
        .expect("memory store schema should initialize");

    let mut thread = Thread::new(ThreadContextLocator::from(&locator), incoming.received_at);
    thread.bind_store(store.clone());
    thread
        .push_message(ChatMessage::new(
            ChatMessageRole::User,
            "persist me",
            incoming.received_at,
        ))
        .await
        .expect("push_message should persist");

    let stored = store
        .load_thread_context(&locator)
        .await
        .expect("stored thread should load")
        .expect("stored thread should exist");
    assert_eq!(stored.snapshot.thread.messages.len(), 1);
    assert_eq!(stored.snapshot.thread.messages[0].content, "persist me");
}

#[tokio::test]
async fn compact_rewrite_replaces_non_system_messages_in_store() {
    // 测试场景: compact 写回应直接替换 persisted message 序列，而不是生成 compacted turn。
    let incoming = build_incoming();
    let locator = build_locator(&incoming);
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    store
        .initialize_schema()
        .await
        .expect("memory store schema should initialize");

    let mut thread = Thread::new(ThreadContextLocator::from(&locator), incoming.received_at);
    thread.bind_store(store.clone());
    thread
        .push_message(ChatMessage::new(
            ChatMessageRole::System,
            "system prompt",
            incoming.received_at,
        ))
        .await
        .expect("system prompt should persist");
    thread
        .push_message(ChatMessage::new(
            ChatMessageRole::User,
            "before compact",
            incoming.received_at,
        ))
        .await
        .expect("user message should persist");
    thread
        .replace_messages_after_compaction(vec![
            ChatMessage::new(
                ChatMessageRole::Assistant,
                "这是压缩后的上下文",
                incoming.received_at,
            ),
            ChatMessage::new(ChatMessageRole::User, "继续", incoming.received_at),
        ])
        .await
        .expect("compact rewrite should persist");

    let stored = store
        .load_thread_context(&locator)
        .await
        .expect("stored thread should load")
        .expect("stored thread should exist");
    assert_eq!(stored.snapshot.thread.messages.len(), 3);
    assert_eq!(
        stored.snapshot.thread.messages[1].content,
        "这是压缩后的上下文"
    );
    assert_eq!(stored.snapshot.thread.messages[2].content, "继续");
}

#[tokio::test]
async fn feature_override_persists_without_request_finalization() {
    // 测试场景: feature state 变更也必须走 thread-owned 原子持久化，不依赖 request finalize。
    let incoming = build_incoming();
    let locator = build_locator(&incoming);
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    store
        .initialize_schema()
        .await
        .expect("memory store schema should initialize");

    let mut thread = Thread::new(ThreadContextLocator::from(&locator), incoming.received_at);
    thread.bind_store(store.clone());
    thread
        .persist_feature_enabled(Feature::AutoCompact, true)
        .await
        .expect("feature override should persist");

    let stored = store
        .load_thread_context(&locator)
        .await
        .expect("stored thread should load")
        .expect("stored thread should exist");
    assert!(
        stored
            .snapshot
            .state
            .features
            .enabled_features
            .contains(Feature::AutoCompact)
    );
    assert!(thread.auto_compact_enabled(false));
}

#[tokio::test]
async fn compact_source_messages_exclude_stable_system_prefix() {
    // 测试场景: runtime compact 只能消费非 system 正式消息，稳定前缀不能进入 compact source。
    let now = Utc::now();
    let mut thread = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_thread",
            "thread_compact",
            "thread_compact",
        ),
        now,
    );
    thread
        .push_message(ChatMessage::new(
            ChatMessageRole::System,
            "system prompt",
            now,
        ))
        .await
        .expect("system prompt should persist");
    thread
        .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
        .await
        .expect("user message should persist");

    let compact_source = thread.compact_source_messages();
    assert_eq!(compact_source.len(), 1);
    assert_eq!(compact_source[0].role, ChatMessageRole::User);
    assert_eq!(compact_source[0].content, "hello");
}

#[test]
fn derive_internal_thread_id_is_stable() {
    // 测试场景: thread-first schema 仍要求 thread_id 从稳定 thread key 可重复派生。
    let thread_id = derive_internal_thread_id("ou_thread:feishu:chat_thread");
    assert_eq!(
        thread_id,
        derive_internal_thread_id("ou_thread:feishu:chat_thread")
    );
}

#[test]
fn thread_agent_kind_profiles_follow_predefined_catalog_defaults() {
    // 测试场景: `ThreadAgentKind` 的初始化真相来自仓库静态 agent catalog，而不是散落的硬编码默认值。
    let main_agent = ThreadAgent::from_kind(ThreadAgentKind::Main);
    let browser_agent = ThreadAgent::from_kind(ThreadAgentKind::Browser);
    let obswiki_agent = ThreadAgent::from_kind(ThreadAgentKind::Obswiki);

    assert_eq!(
        ThreadAgentKind::Main.system_prompt(),
        DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim()
    );
    assert_eq!(
        ThreadAgentKind::Browser.system_prompt(),
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim()
    );
    assert!(main_agent.bound_toolsets.is_empty());
    assert_eq!(browser_agent.bound_toolsets, vec!["browser".to_string()]);
    assert_eq!(obswiki_agent.bound_toolsets, vec!["obswiki".to_string()]);
}

#[test]
fn child_thread_identity_uses_parent_and_profile_as_stable_key() {
    // 测试场景: child thread id 只由 parent_thread_id + subagent_key 决定，spawn_mode 不会派生第二个实例。
    let persist =
        ThreadContextLocator::new(None, "feishu", "ou_thread", "chat_thread", "child-thread")
            .with_child_thread(ChildThreadIdentity::new(
                "parent-thread",
                "browser",
                SubagentSpawnMode::Persist,
            ));
    let yolo =
        ThreadContextLocator::new(None, "feishu", "ou_thread", "chat_thread", "child-thread")
            .with_child_thread(ChildThreadIdentity::new(
                "parent-thread",
                "browser",
                SubagentSpawnMode::Yolo,
            ));

    assert_eq!(persist.thread_key(), yolo.thread_key());
    assert_eq!(
        derive_child_thread_id("parent-thread", "browser"),
        derive_child_thread_id("parent-thread", "browser")
    );
}
