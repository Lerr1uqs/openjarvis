use super::support::ThreadTestExt;
use super::{SqliteFixture, build_compacted_thread_context, build_incoming, build_locator};
use chrono::Utc;
use openjarvis::session::{
    ExternalMessageDedupRecord, SessionStore, SessionStoreError, SessionStoreResult,
    SqliteSessionStore,
};

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
    let thread_context = build_compacted_thread_context(&locator, now);
    let dedup_record = ExternalMessageDedupRecord {
        thread_id: locator.thread_id,
        external_message_id: "msg_sqlite_store".to_string(),
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

    assert_eq!(loaded.non_system_messages().len(), 2);
    assert_eq!(
        loaded.non_system_messages()[0].content,
        "这是压缩后的上下文"
    );
    assert_eq!(loaded.non_system_messages()[1].content, "继续");
    assert_eq!(loaded.load_toolsets(), vec!["demo".to_string()]);
    assert!(loaded.auto_compact_enabled(false));
    assert_eq!(loaded_dedup.completed_at, now);
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
    let thread_context = build_compacted_thread_context(&locator, now);

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
