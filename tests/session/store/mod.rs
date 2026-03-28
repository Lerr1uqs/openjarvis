use chrono::{DateTime, Utc};
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    model::{IncomingMessage, ReplyTarget},
    session::{SessionKey, ThreadLocator},
    thread::{ThreadContext, ThreadContextLocator},
};
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

mod memory;
mod sqlite;

fn build_incoming(external_message_id: &str, external_thread_id: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(external_message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_store".to_string(),
        user_name: None,
        content: "hello".to_string(),
        external_thread_id: Some(external_thread_id.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_store".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_locator(session_id: Uuid, incoming: &IncomingMessage) -> ThreadLocator {
    let session_key = SessionKey::from_incoming(incoming);
    let external_thread_id = incoming.resolved_external_thread_id();
    let thread_id = session_key.derive_thread_id(&external_thread_id);
    ThreadLocator::new(session_id, incoming, external_thread_id, thread_id)
}

fn build_compacted_thread_context(
    locator: &ThreadLocator,
    now: DateTime<Utc>,
) -> (ThreadContext, Uuid) {
    let mut thread_context = ThreadContext::new(ThreadContextLocator::from(locator), now);
    thread_context.enable_auto_compact();
    let turn_id = thread_context.store_turn_state(
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
    (thread_context, turn_id)
}

struct SqliteFixture {
    root: PathBuf,
    db_path: PathBuf,
}

impl SqliteFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("sqlite fixture root should be created");
        let db_path = root.join("session.sqlite3");
        Self { root, db_path }
    }

    fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl Drop for SqliteFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
