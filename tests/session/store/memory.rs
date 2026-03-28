use super::{build_compacted_thread_context, build_incoming, build_locator};
use chrono::Utc;
use openjarvis::session::{
    ExternalMessageDedupRecord, MemorySessionStore, SessionStore, SessionStoreError,
};

#[tokio::test]
async fn memory_store_roundtrips_thread_snapshot_and_dedup_record() {
    // 测试场景: memory store 要复用和持久化 backend 相同的接口语义，能完整 roundtrip 线程快照和 dedup 记录。
    let store = MemorySessionStore::new();
    store
        .initialize_schema()
        .await
        .expect("memory schema initialization should succeed");
    let incoming = build_incoming("msg_memory_store", "thread_memory_store");
    let session = store
        .resolve_or_create_session(
            &openjarvis::session::SessionKey::from_incoming(&incoming),
            Utc::now(),
        )
        .await
        .expect("session should resolve");
    let locator = build_locator(session.id, &incoming);
    let now = Utc::now();
    let (thread_context, turn_id) = build_compacted_thread_context(&locator, now);
    let dedup_record = ExternalMessageDedupRecord {
        thread_id: locator.thread_id,
        external_message_id: "msg_memory_store".to_string(),
        turn_id: Some(turn_id),
        completed_at: now,
    };

    store
        .save_thread_context(&thread_context, now, Some(&dedup_record))
        .await
        .expect("thread snapshot should save");

    let loaded = store
        .load_thread_context(&locator)
        .await
        .expect("thread snapshot should load")
        .expect("thread snapshot should exist");
    let loaded_dedup = store
        .load_external_message_record(&locator, "msg_memory_store")
        .await
        .expect("dedup record should load")
        .expect("dedup record should exist");

    assert_eq!(loaded.turns.len(), 1);
    assert_eq!(loaded.turns[0].messages[0].content, "这是压缩后的上下文");
    assert_eq!(loaded.turns[0].messages[1].content, "继续");
    assert_eq!(loaded.load_toolsets(), vec!["demo".to_string()]);
    assert!(loaded.auto_compact_enabled(false));
    assert_eq!(loaded_dedup.turn_id, Some(turn_id));
}

#[tokio::test]
async fn memory_store_rejects_stale_revision_writes() {
    // 测试场景: 同一个线程的旧快照不能覆盖更新后的 revision，memory store 也必须实现 CAS 语义。
    let store = MemorySessionStore::new();
    let incoming = build_incoming("msg_memory_conflict", "thread_memory_conflict");
    let session = store
        .resolve_or_create_session(
            &openjarvis::session::SessionKey::from_incoming(&incoming),
            Utc::now(),
        )
        .await
        .expect("session should resolve");
    let locator = build_locator(session.id, &incoming);
    let now = Utc::now();
    let (thread_context, _turn_id) = build_compacted_thread_context(&locator, now);

    store
        .save_thread_context(&thread_context, now, None)
        .await
        .expect("initial snapshot should save");
    let stale = store
        .load_thread_context(&locator)
        .await
        .expect("stale snapshot should load")
        .expect("stale snapshot should exist");
    let mut fresh = stale.clone();
    fresh.enable_auto_compact();
    store
        .save_thread_context(&fresh, Utc::now(), None)
        .await
        .expect("fresh snapshot should save");

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
}
