use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    model::{IncomingMessage, ReplyTarget},
    session::{
        SessionStore, SessionStoreError, SessionStoreResult, SqliteSessionStore, ThreadLocator,
    },
    thread::{PersistedThreadSnapshot, ThreadContextLocator, ThreadSnapshotStore},
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

struct SqliteFixture {
    path: std::path::PathBuf,
}

impl SqliteFixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{name}-{}.db", Uuid::new_v4()));
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        Self { path }
    }

    fn db_path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for SqliteFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn build_incoming() -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_sqlite_store".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_sqlite".to_string(),
        user_name: None,
        content: "hello".to_string(),
        external_thread_id: Some("thread_sqlite".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_sqlite".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_locator(incoming: &IncomingMessage) -> ThreadLocator {
    let session_key = openjarvis::session::SessionKey::from_incoming(incoming);
    ThreadLocator::new(
        session_key.derive_session_id(),
        incoming,
        incoming.resolved_external_thread_id(),
        session_key.derive_thread_id(&incoming.resolved_external_thread_id()),
    )
}

#[tokio::test]
async fn sqlite_store_roundtrips_thread_snapshot() -> SessionStoreResult<()> {
    // 测试场景: sqlite store 要能恢复 thread-first snapshot 与 revision。
    let fixture = SqliteFixture::new("openjarvis-sqlite-store-roundtrip");
    let store = SqliteSessionStore::open(fixture.db_path()).await?;
    store.initialize_schema().await?;
    let incoming = build_incoming();
    let locator = build_locator(&incoming);
    let snapshot = PersistedThreadSnapshot {
        thread: openjarvis::thread::ThreadContext {
            messages: vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                "hello sqlite",
                incoming.received_at,
            )],
            created_at: incoming.received_at,
            updated_at: incoming.received_at,
        },
        state: openjarvis::thread::ThreadState::default(),
    };

    store
        .save_thread_snapshot(&ThreadContextLocator::from(&locator), &snapshot, 0)
        .await
        .expect("snapshot should persist");

    let loaded = store
        .load_thread_context(&locator)
        .await?
        .expect("snapshot should load");
    assert_eq!(loaded.revision, 1);
    assert_eq!(loaded.snapshot.thread.messages.len(), 1);
    assert_eq!(loaded.snapshot.thread.messages[0].content, "hello sqlite");
    Ok(())
}

#[tokio::test]
async fn sqlite_store_rejects_stale_revision_writes() -> SessionStoreResult<()> {
    // 测试场景: sqlite store 必须用 revision/CAS 阻止旧快照覆盖新状态。
    let fixture = SqliteFixture::new("openjarvis-sqlite-store-conflict");
    let store: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::open(fixture.db_path()).await?);
    store.initialize_schema().await?;
    let incoming = build_incoming();
    let locator = build_locator(&incoming);
    let mut snapshot = PersistedThreadSnapshot::new(incoming.received_at);
    snapshot.thread.messages.push(ChatMessage::new(
        ChatMessageRole::User,
        "v1",
        incoming.received_at,
    ));

    store
        .save_thread_snapshot(&ThreadContextLocator::from(&locator), &snapshot, 0)
        .await
        .expect("initial snapshot should save");
    store
        .save_thread_snapshot(&ThreadContextLocator::from(&locator), &snapshot, 1)
        .await
        .expect("fresh snapshot should save");

    let error = store
        .save_thread_snapshot(&ThreadContextLocator::from(&locator), &snapshot, 0)
        .await
        .expect_err("stale snapshot should conflict");
    let conflict = error
        .downcast_ref::<SessionStoreError>()
        .expect("error should downcast to SessionStoreError");
    match conflict {
        SessionStoreError::RevisionConflict(conflict) => {
            assert_eq!(conflict.thread_id, locator.thread_id.to_string());
            assert_eq!(conflict.expected_revision, 0);
            assert_eq!(conflict.actual_revision, 2);
        }
        other => panic!("unexpected store error: {other:?}"),
    }
    Ok(())
}
