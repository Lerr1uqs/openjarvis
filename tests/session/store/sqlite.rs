use super::{SqliteFixture, build_compacted_thread_context, build_incoming, build_locator};
use chrono::Utc;
use openjarvis::session::{
    ExternalMessageDedupRecord, SessionStore, SessionStoreError, SessionStoreResult,
    SqliteSessionStore,
};
use rusqlite::{Connection, params};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn sqlite_store_roundtrips_thread_snapshot_and_dedup_record() -> SessionStoreResult<()> {
    // 测试场景: SQLite store 要能把 compact turn、线程状态和 dedup 记录一起落盘并恢复。
    let fixture = SqliteFixture::new("openjarvis-sqlite-store-roundtrip");
    let store = SqliteSessionStore::open(fixture.db_path()).await?;
    store.initialize_schema().await?;
    let incoming = build_incoming("msg_sqlite_store", "thread_sqlite_store");
    let session = store
        .resolve_or_create_session(
            &openjarvis::session::SessionKey::from_incoming(&incoming),
            Utc::now(),
        )
        .await?;
    let locator = build_locator(session.id, &incoming);
    let now = Utc::now();
    let (thread_context, turn_id) = build_compacted_thread_context(&locator, now);
    let dedup_record = ExternalMessageDedupRecord {
        thread_id: locator.thread_id,
        external_message_id: "msg_sqlite_store".to_string(),
        turn_id: Some(turn_id),
        completed_at: now,
    };

    store
        .save_thread_context(&thread_context, now, Some(&dedup_record))
        .await?;

    let loaded = store
        .load_thread_context(&locator)
        .await?
        .expect("thread snapshot should exist");
    let loaded_dedup = store
        .load_external_message_record(&locator, "msg_sqlite_store")
        .await?
        .expect("dedup record should exist");

    assert_eq!(loaded.load_messages().len(), 2);
    assert_eq!(loaded.load_messages()[0].content, "这是压缩后的上下文");
    assert_eq!(loaded.load_messages()[1].content, "继续");
    assert_eq!(loaded.load_toolsets(), vec!["demo".to_string()]);
    assert!(loaded.auto_compact_enabled(false));
    assert_eq!(loaded_dedup.turn_id, Some(turn_id));
    Ok(())
}

#[tokio::test]
async fn sqlite_store_rejects_stale_revision_writes() -> SessionStoreResult<()> {
    // 测试场景: SQLite store 必须用 revision/CAS 阻止旧快照覆盖新状态。
    let fixture = SqliteFixture::new("openjarvis-sqlite-store-conflict");
    let store = SqliteSessionStore::open(fixture.db_path()).await?;
    store.initialize_schema().await?;
    let incoming = build_incoming("msg_sqlite_conflict", "thread_sqlite_conflict");
    let session = store
        .resolve_or_create_session(
            &openjarvis::session::SessionKey::from_incoming(&incoming),
            Utc::now(),
        )
        .await?;
    let locator = build_locator(session.id, &incoming);
    let now = Utc::now();
    let (thread_context, _turn_id) = build_compacted_thread_context(&locator, now);

    store
        .save_thread_context(&thread_context, now, None)
        .await?;
    let stale = store
        .load_thread_context(&locator)
        .await?
        .expect("stale snapshot should exist");
    let mut fresh = stale.clone();
    fresh.disable_auto_compact();
    store.save_thread_context(&fresh, Utc::now(), None).await?;

    let error = store
        .save_thread_context(&stale, Utc::now(), None)
        .await
        .expect_err("stale snapshot should conflict");

    match error {
        SessionStoreError::RevisionConflict(conflict) => {
            assert_eq!(conflict.thread_id, locator.thread_id.to_string());
            assert_eq!(conflict.expected_revision + 1, conflict.actual_revision);
        }
        other => panic!("unexpected store error: {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn sqlite_store_loads_legacy_thread_context_snapshot() -> SessionStoreResult<()> {
    // 测试场景: 旧版 sqlite snapshot 仍然使用 conversation/request_context 结构时，
    // 当前线程模型也必须能恢复 system prompt、消息历史、toolset 和 tool event。
    let fixture = SqliteFixture::new("openjarvis-sqlite-store-legacy-snapshot");
    let store = SqliteSessionStore::open(fixture.db_path()).await?;
    store.initialize_schema().await?;
    let incoming = build_incoming("msg_sqlite_legacy", "thread_sqlite_legacy");
    let session = store
        .resolve_or_create_session(
            &openjarvis::session::SessionKey::from_incoming(&incoming),
            Utc::now(),
        )
        .await?;
    let locator = build_locator(session.id, &incoming);
    let started_at = Utc::now();
    let completed_at = started_at + chrono::Duration::seconds(5);
    let turn_id = Uuid::new_v4();
    let tool_event_id = Uuid::new_v4();
    let snapshot_json = serde_json::to_string(&json!({
        "locator": {
            "session_id": session.id.to_string(),
            "channel": locator.channel,
            "user_id": locator.user_id,
            "external_thread_id": locator.external_thread_id,
            "thread_id": locator.thread_id.to_string(),
        },
        "conversation": {
            "external_thread_id": locator.external_thread_id,
            "turns": [{
                "id": turn_id,
                "external_message_id": "msg_sqlite_legacy",
                "messages": [
                    {
                        "role": "User",
                        "content": "legacy hello",
                        "created_at": started_at,
                    },
                    {
                        "role": "Assistant",
                        "content": "legacy world",
                        "created_at": completed_at,
                    }
                ],
                "started_at": started_at,
                "completed_at": completed_at,
            }],
            "tool_events": [{
                "id": tool_event_id,
                "turn_id": turn_id,
                "kind": "LoadToolset",
                "toolset_name": "browser",
                "tool_name": "load_toolset",
                "metadata": {},
                "is_error": false,
                "recorded_at": completed_at,
            }],
            "created_at": started_at,
            "updated_at": completed_at,
        },
        "state": {
            "features": {
                "auto_compact_override": true,
            },
            "request_context": {
                "system": [{
                    "role": "System",
                    "content": "legacy system prompt",
                    "created_at": started_at,
                }],
            },
            "tools": {
                "loaded_toolsets": ["browser"],
            },
            "approval": {
                "pending": [],
                "decisions": [],
            },
        },
    }))
    .expect("legacy snapshot json should serialize");

    let connection =
        Connection::open(fixture.db_path()).expect("legacy snapshot sqlite connection should open");
    connection
        .execute(
            r#"
INSERT INTO thread_metadata (
    thread_id,
    session_id,
    external_thread_id,
    revision,
    snapshot_json,
    created_at,
    updated_at
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
            params![
                locator.thread_id.to_string(),
                session.id.to_string(),
                locator.external_thread_id,
                3_i64,
                snapshot_json,
                started_at.to_rfc3339(),
                completed_at.to_rfc3339(),
            ],
        )
        .expect("legacy snapshot row should insert");

    let restored = store
        .load_thread_context(&locator)
        .await?
        .expect("legacy thread snapshot should load");

    assert_eq!(restored.system_prefix_messages().len(), 1);
    assert_eq!(
        restored.system_prefix_messages()[0].content,
        "legacy system prompt"
    );
    assert_eq!(restored.load_messages().len(), 2);
    assert_eq!(restored.load_messages()[0].content, "legacy hello");
    assert_eq!(restored.load_messages()[1].content, "legacy world");
    assert_eq!(restored.load_toolsets(), vec!["browser".to_string()]);
    assert!(restored.auto_compact_enabled(false));
    assert_eq!(restored.load_tool_events().len(), 1);
    assert_eq!(
        restored.load_tool_events()[0].toolset_name.as_deref(),
        Some("browser")
    );
    assert_eq!(restored.load_tool_events()[0].turn_id, Some(turn_id));
    Ok(())
}
