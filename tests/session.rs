use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
        ToolsetCatalogEntry, empty_tool_input_schema,
    },
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    model::{IncomingMessage, ReplyTarget},
    session::{
        MemorySessionStore, SessionKey, SessionManager, SessionStore, SessionStrategy,
        SqliteSessionStore,
    },
    thread::{
        ThreadContext, ThreadContextLocator, ThreadToolEvent, ThreadToolEventKind,
        derive_internal_thread_id,
    },
};
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

struct DemoSessionTool;

struct SessionSqliteFixture {
    root: PathBuf,
    db_path: PathBuf,
}

impl SessionSqliteFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("session sqlite fixture root should be created");
        let db_path = root.join("session.sqlite3");
        Self { root, db_path }
    }

    fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl Drop for SessionSqliteFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[async_trait]
impl ToolHandler for DemoSessionTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo from the demo session toolset".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "session-demo".to_string(),
            metadata: json!({ "toolset": "demo" }),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn store_and_load_turn_creates_session_state() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_1", "hello");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            vec![
                ChatMessage::new(ChatMessageRole::User, "hello", incoming.received_at),
                ChatMessage::new(ChatMessageRole::Assistant, "world", Utc::now()),
            ],
            incoming.received_at,
            Utc::now(),
        )
        .await
        .expect("turn should store");

    let history = manager.load_turn(&locator).await.expect("history should load");
    let session = manager
        .get_session(&SessionKey::from_incoming(&incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("default thread should exist");

    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.external_thread_id, "default");
    assert_eq!(history.len(), 2);
    assert_eq!(
        thread.turns[0]
            .final_assistant_message()
            .map(|message| message.content.as_str()),
        Some("world")
    );
}

#[tokio::test]
async fn load_turn_keeps_tool_call_id_history() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_1", "read config");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            vec![
                ChatMessage::new(ChatMessageRole::User, "read config", incoming.received_at),
                ChatMessage::new(ChatMessageRole::Assistant, "", Utc::now()).with_tool_calls(vec![
                    ChatToolCall {
                        id: "call_1".to_string(),
                        name: "read".to_string(),
                        arguments: json!({ "path": "config.yaml" }),
                    },
                ]),
                ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now())
                    .with_tool_call_id("call_1"),
                ChatMessage::new(ChatMessageRole::Assistant, "done", Utc::now()),
            ],
            incoming.received_at,
            Utc::now(),
        )
        .await
        .expect("turn should store");

    let history = manager.load_turn(&locator).await.expect("history should load");

    assert!(
        history
            .iter()
            .any(|message| message.tool_calls.iter().any(|call| call.id == "call_1"))
    );
    assert!(
        history
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call_1"))
    );
}

#[tokio::test]
async fn strategy_keeps_only_latest_five_messages_per_thread() {
    let manager = SessionManager::with_strategy(SessionStrategy {
        max_messages_per_thread: 5,
    });
    let incoming = build_incoming("msg_1", "trim this");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            (0..7)
                .map(|index| {
                    ChatMessage::new(
                        ChatMessageRole::Assistant,
                        format!("message_{index}"),
                        Utc::now(),
                    )
                })
                .collect(),
            incoming.received_at,
            Utc::now(),
        )
        .await
        .expect("first turn should store");
    manager
        .store_turn(
            &locator,
            Some("msg_2".to_string()),
            (7..10)
                .map(|index| {
                    ChatMessage::new(
                        ChatMessageRole::Assistant,
                        format!("message_{index}"),
                        Utc::now(),
                    )
                })
                .collect(),
            incoming.received_at,
            Utc::now(),
        )
        .await
        .expect("second turn should store");

    let history = manager.load_turn(&locator).await.expect("history should load");
    let session = manager
        .get_session(&SessionKey::from_incoming(&incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("default thread should exist");

    assert_eq!(history.len(), 5);
    assert_eq!(
        history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "message_5".to_string(),
            "message_6".to_string(),
            "message_7".to_string(),
            "message_8".to_string(),
            "message_9".to_string(),
        ]
    );
    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.turns[0].messages.len(), 2);
    assert_eq!(thread.turns[1].messages.len(), 3);
    assert_eq!(thread.turns[0].messages[0].content, "message_5");
    assert_eq!(thread.turns[1].messages[2].content, "message_9");
}

#[tokio::test]
async fn load_or_create_thread_reuses_internal_uuid_for_same_external_thread() {
    let manager = SessionManager::new();
    let first_incoming = build_incoming_with_thread("msg_1", "hello", Some("ext_thread_1"));
    let second_incoming = build_incoming_with_thread("msg_2", "world", Some("ext_thread_1"));

    let first_locator = manager
        .load_or_create_thread(&first_incoming)
        .await
        .expect("first thread should resolve");
    let second_locator = manager
        .load_or_create_thread(&second_incoming)
        .await
        .expect("second thread should resolve");

    assert_eq!(first_locator.session_id, second_locator.session_id);
    assert_eq!(first_locator.external_thread_id, "ext_thread_1");
    assert_eq!(second_locator.external_thread_id, "ext_thread_1");
    assert_eq!(first_locator.thread_id, second_locator.thread_id);
    assert_eq!(
        first_locator.thread_id,
        derive_internal_thread_id("ou_xxx:feishu:ext_thread_1")
    );
}

#[tokio::test]
async fn store_turn_with_state_persists_loaded_toolsets_and_tool_events() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_tool_state", "hello tool state");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let tool_event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, Utc::now());
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };

    manager
        .store_turn_with_state(
            &locator,
            incoming.external_message_id.clone(),
            vec![ChatMessage::new(
                ChatMessageRole::User,
                "hello tool state",
                incoming.received_at,
            )],
            incoming.received_at,
            Utc::now(),
            vec!["demo".to_string()],
            vec![tool_event],
        )
        .await
        .expect("turn state should store");

    let thread_state = manager
        .load_thread_state(&locator)
        .await
        .expect("thread state should load");

    assert_eq!(thread_state.loaded_toolsets, vec!["demo".to_string()]);
    assert_eq!(thread_state.tool_events.len(), 1);
    assert_eq!(
        thread_state.tool_events[0].toolset_name.as_deref(),
        Some("demo")
    );
    assert!(thread_state.tool_events[0].turn_id.is_some());
}

#[tokio::test]
async fn store_and_load_thread_context_roundtrips_runtime_state() {
    // 测试场景: Session 新的 ThreadContext 读写接口要能完整保留线程状态，而不是退回旧 thread shape。
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_thread_context", "hello context");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let now = Utc::now();
    let tool_event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };
    let mut thread_context = ThreadContext::new(ThreadContextLocator::from(&locator), now);
    thread_context.enable_auto_compact();
    thread_context.store_turn_state(
        incoming.external_message_id.clone(),
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "hello context",
            incoming.received_at,
        )],
        incoming.received_at,
        now,
        vec!["demo".to_string()],
        vec![tool_event],
    );

    manager
        .store_thread_context(&locator, thread_context, now)
        .await
        .expect("thread context should store");

    let loaded = manager
        .load_thread_context(&locator)
        .await
        .expect("thread context should load")
        .expect("thread context should be stored");
    let thread_state = manager
        .load_thread_state(&locator)
        .await
        .expect("thread state should load");

    assert_eq!(loaded.locator, ThreadContextLocator::from(&locator));
    assert_eq!(loaded.load_toolsets(), vec!["demo".to_string()]);
    assert!(loaded.compact_enabled(false));
    assert!(loaded.auto_compact_enabled(false));
    assert_eq!(loaded.load_tool_events().len(), 1);
    assert!(thread_state.thread_context.is_some());
    assert_eq!(thread_state.loaded_toolsets, vec!["demo".to_string()]);
}

#[tokio::test]
async fn load_thread_state_can_rehydrate_runtime_by_internal_thread_id() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_rehydrate", "rehydrate demo");
    let locator = manager
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    manager
        .store_turn_with_state(
            &locator,
            incoming.external_message_id.clone(),
            vec![ChatMessage::new(
                ChatMessageRole::User,
                "rehydrate demo",
                incoming.received_at,
            )],
            incoming.received_at,
            Utc::now(),
            vec!["demo".to_string()],
            Vec::new(),
        )
        .await
        .expect("turn state should store");

    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo runtime reconstruction toolset"),
            vec![Arc::new(DemoSessionTool)],
        )
        .await
        .expect("demo toolset should register");

    let thread_state = manager
        .load_thread_state(&locator)
        .await
        .expect("thread state should load");
    registry
        .rehydrate_thread(
            &locator.thread_id.to_string(),
            &thread_state.loaded_toolsets,
        )
        .await;
    let visible_tools = registry
        .list_for_thread(&locator.thread_id.to_string())
        .await
        .expect("rehydrated runtime should expose loaded toolset tools");

    assert!(visible_tools.iter().any(|tool| tool.name == "demo__echo"));
}

#[tokio::test]
async fn loaded_toolsets_remain_isolated_between_internal_threads_for_same_user() {
    let manager = SessionManager::new();
    let first_incoming = build_incoming_with_thread("msg_a", "hello a", Some("thread_a"));
    let second_incoming = build_incoming_with_thread("msg_b", "hello b", Some("thread_b"));
    let first_locator = manager
        .load_or_create_thread(&first_incoming)
        .await
        .expect("first thread should resolve");
    let second_locator = manager
        .load_or_create_thread(&second_incoming)
        .await
        .expect("second thread should resolve");

    manager
        .store_turn_with_state(
            &first_locator,
            first_incoming.external_message_id.clone(),
            vec![ChatMessage::new(
                ChatMessageRole::User,
                "hello a",
                first_incoming.received_at,
            )],
            first_incoming.received_at,
            Utc::now(),
            vec!["demo_a".to_string()],
            Vec::new(),
        )
        .await
        .expect("first thread state should store");
    manager
        .store_turn_with_state(
            &second_locator,
            second_incoming.external_message_id.clone(),
            vec![ChatMessage::new(
                ChatMessageRole::User,
                "hello b",
                second_incoming.received_at,
            )],
            second_incoming.received_at,
            Utc::now(),
            vec!["demo_b".to_string()],
            Vec::new(),
        )
        .await
        .expect("second thread state should store");

    let first_state = manager
        .load_thread_state(&first_locator)
        .await
        .expect("first thread state should load");
    let second_state = manager
        .load_thread_state(&second_locator)
        .await
        .expect("second thread state should load");

    assert_eq!(first_state.loaded_toolsets, vec!["demo_a".to_string()]);
    assert_eq!(second_state.loaded_toolsets, vec!["demo_b".to_string()]);
}

#[tokio::test]
async fn store_turn_with_active_thread_replaces_old_history_before_appending_new_turn() {
    // 测试场景: compact 已经替换 active history 后，session 应写回 compacted turn，再追加本轮新 turn。
    let manager = SessionManager::new();
    let first_incoming = build_incoming("msg_compact_1", "before compact");
    let locator = manager
        .load_or_create_thread(&first_incoming)
        .await
        .expect("thread should resolve");
    manager
        .store_turn(
            &locator,
            first_incoming.external_message_id.clone(),
            vec![
                ChatMessage::new(
                    ChatMessageRole::User,
                    "before compact",
                    first_incoming.received_at,
                ),
                ChatMessage::new(ChatMessageRole::Assistant, "old reply", Utc::now()),
            ],
            first_incoming.received_at,
            Utc::now(),
        )
        .await
        .expect("initial turn should store");

    let mut compacted_thread = manager
        .load_thread_state(&locator)
        .await
        .expect("thread state should load")
        .thread
        .expect("thread should exist before compact");
    compacted_thread.turns = vec![openjarvis::thread::ConversationTurn::new(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", Utc::now()),
            ChatMessage::new(ChatMessageRole::User, "继续", Utc::now()),
        ],
        Utc::now(),
        Utc::now(),
    )];

    manager
        .store_turn_with_active_thread(
            &locator,
            Some(compacted_thread),
            Some("msg_compact_2".to_string()),
            vec![
                ChatMessage::new(ChatMessageRole::User, "new question", Utc::now()),
                ChatMessage::new(ChatMessageRole::Assistant, "new reply", Utc::now()),
            ],
            Utc::now(),
            Utc::now(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("compacted turn should store");

    let session = manager
        .get_session(&SessionKey::from_incoming(&first_incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.turns[0].messages[0].content, "这是压缩后的上下文");
    assert_eq!(thread.turns[0].messages[1].content, "继续");
    assert_eq!(thread.turns[1].messages[0].content, "new question");
    assert_eq!(thread.turns[1].messages[1].content, "new reply");
}

#[tokio::test]
async fn shared_store_merges_stale_state_updates_and_restores_external_message_dedup() {
    // 测试场景: 多个 SessionManager 共享同一个 store 时，旧线程状态写入要和最新快照合并，而不是覆盖 turn；dedup 记录也要能被新 manager 恢复。
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let manager_a = SessionManager::with_store(Arc::clone(&store), SessionStrategy::default())
        .await
        .expect("manager A should build");
    let manager_b = SessionManager::with_store(Arc::clone(&store), SessionStrategy::default())
        .await
        .expect("manager B should build");
    let incoming = build_incoming_with_thread("msg_conflict_1", "conflict", Some("thread_conflict"));
    let locator_a = manager_a
        .load_or_create_thread(&incoming)
        .await
        .expect("manager A should resolve thread");
    let locator_b = manager_b
        .load_or_create_thread(&incoming)
        .await
        .expect("manager B should resolve thread");
    let stale = manager_b
        .load_thread_context(&locator_b)
        .await
        .expect("manager B should load thread context")
        .expect("thread context should exist");
    let mut fresh = manager_a
        .load_thread_context(&locator_a)
        .await
        .expect("manager A should load thread context")
        .expect("thread context should exist");
    fresh.enable_auto_compact();
    manager_a
        .store_thread_context(&locator_a, fresh, Utc::now())
        .await
        .expect("fresh context should store");

    manager_b
        .store_thread_context(&locator_b, stale, Utc::now())
        .await
        .expect("stale state update should merge with latest snapshot");
    let merged = manager_b
        .load_thread_context(&locator_b)
        .await
        .expect("merged context should load")
        .expect("merged context should exist");
    assert!(merged.auto_compact_enabled(false));

    manager_a
        .store_turn(
            &locator_a,
            Some("msg_conflict_2".to_string()),
            vec![
                ChatMessage::new(ChatMessageRole::User, "after conflict", Utc::now()),
                ChatMessage::new(ChatMessageRole::Assistant, "stored once", Utc::now()),
            ],
            Utc::now(),
            Utc::now(),
        )
        .await
        .expect("dedup turn should store");

    let manager_c = SessionManager::with_store(Arc::clone(&store), SessionStrategy::default())
        .await
        .expect("manager C should build");
    let locator_c = manager_c
        .load_or_create_thread(&incoming)
        .await
        .expect("manager C should resolve thread");

    assert!(
        manager_c
            .is_external_message_processed(&locator_c, "msg_conflict_2")
            .await
            .expect("dedup record should load")
    );
}

#[tokio::test]
async fn sqlite_backed_session_manager_restores_compact_turn_toolsets_and_follow_up_turns() {
    // 测试场景: SQLite 持久化后的线程在“重启”后要恢复 compact turn、loaded toolsets、/auto-compact 状态，并继续同线程追加新 turn。
    let fixture = SessionSqliteFixture::new("openjarvis-session-restart");
    let store_a: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open(fixture.db_path())
            .await
            .expect("sqlite store A should open"),
    );
    let manager_a = SessionManager::with_store(
        Arc::clone(&store_a),
        SessionStrategy {
            max_messages_per_thread: usize::MAX,
        },
    )
    .await
    .expect("manager A should build");
    let incoming = build_incoming_with_thread("msg_restart_1", "restore me", Some("thread_restart"));
    let locator_a = manager_a
        .load_or_create_thread(&incoming)
        .await
        .expect("manager A should resolve thread");
    let now = Utc::now();
    let mut thread_context = ThreadContext::new(ThreadContextLocator::from(&locator_a), now);
    thread_context.enable_auto_compact();
    thread_context.store_turn_state(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ],
        now,
        now,
        vec!["demo".to_string()],
        Vec::new(),
    );
    manager_a
        .store_thread_context(&locator_a, thread_context, now)
        .await
        .expect("initial sqlite-backed context should store");

    let store_b: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open(fixture.db_path())
            .await
            .expect("sqlite store B should open"),
    );
    let manager_b = SessionManager::with_store(
        Arc::clone(&store_b),
        SessionStrategy {
            max_messages_per_thread: usize::MAX,
        },
    )
    .await
    .expect("manager B should build");
    let locator_b = manager_b
        .load_or_create_thread(&incoming)
        .await
        .expect("manager B should resolve thread");
    let restored = manager_b
        .load_thread_context(&locator_b)
        .await
        .expect("restored thread context should load")
        .expect("restored thread context should exist");

    assert_eq!(restored.turns.len(), 1);
    assert_eq!(restored.turns[0].messages[0].content, "这是压缩后的上下文");
    assert_eq!(restored.turns[0].messages[1].content, "继续");
    assert_eq!(restored.load_toolsets(), vec!["demo".to_string()]);
    assert!(restored.auto_compact_enabled(false));

    manager_b
        .store_turn(
            &locator_b,
            Some("msg_restart_2".to_string()),
            vec![
                ChatMessage::new(ChatMessageRole::User, "follow-up", Utc::now()),
                ChatMessage::new(ChatMessageRole::Assistant, "reply-after-restart", Utc::now()),
            ],
            Utc::now(),
            Utc::now(),
        )
        .await
        .expect("follow-up turn should store after restart");
    let history = manager_b
        .load_turn(&locator_b)
        .await
        .expect("history should load after restart");

    assert_eq!(
        history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "这是压缩后的上下文".to_string(),
            "继续".to_string(),
            "follow-up".to_string(),
            "reply-after-restart".to_string(),
        ]
    );
}

fn build_incoming(message_id: &str, content: &str) -> IncomingMessage {
    build_incoming_with_thread(message_id, content, None)
}

fn build_incoming_with_thread(
    message_id: &str,
    content: &str,
    thread_id: Option<&str>,
) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: thread_id.map(|value| value.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}
