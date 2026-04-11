//! In-memory `SessionStore` implementation used by tests and non-persistent runtimes.

use super::{
    ExternalMessageDedupRecord, SessionRevisionConflict, SessionStore, SessionStoreResult,
    StoredSessionRecord,
};
use crate::{
    session::{SessionKey, ThreadLocator},
    thread::Thread,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::Mutex;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Default)]
struct MemoryStoreState {
    sessions: HashMap<SessionKey, StoredSessionRecord>,
    threads: HashMap<Uuid, Thread>,
    external_messages: HashMap<(Uuid, String), ExternalMessageDedupRecord>,
}

/// In-memory store backend that keeps the same persistence contract as SQLite.
#[derive(Debug, Default)]
pub struct MemorySessionStore {
    state: Mutex<MemoryStoreState>,
}

impl MemorySessionStore {
    /// Create an empty memory-backed session store.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::store::MemorySessionStore;
    ///
    /// let store = MemorySessionStore::new();
    /// let _ = store;
    /// ```
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn initialize_schema(&self) -> SessionStoreResult<()> {
        Ok(())
    }

    async fn load_session(
        &self,
        key: &SessionKey,
    ) -> SessionStoreResult<Option<StoredSessionRecord>> {
        let state = self.state.lock().await;
        Ok(state.sessions.get(key).cloned())
    }

    async fn resolve_or_create_session(
        &self,
        key: &SessionKey,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<StoredSessionRecord> {
        let mut state = self.state.lock().await;
        let record = state
            .sessions
            .entry(key.clone())
            .or_insert_with(|| StoredSessionRecord {
                id: Uuid::new_v4(),
                key: key.clone(),
                created_at: now,
                updated_at: now,
            });
        record.updated_at = now;
        Ok(record.clone())
    }

    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<Thread>> {
        let state = self.state.lock().await;
        Ok(state.threads.get(&locator.thread_id).cloned())
    }

    async fn save_thread_context(
        &self,
        thread_context: &Thread,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<u64> {
        let session_id = thread_context
            .locator
            .session_id
            .as_deref()
            .context("thread context session_id is required before saving")?;
        let session_id = Uuid::parse_str(session_id)
            .with_context(|| format!("invalid session_id `{session_id}` on thread context"))?;
        let session_key = SessionKey {
            channel: thread_context.locator.channel.clone(),
            user_id: thread_context.locator.user_id.clone(),
        };
        let thread_id = Uuid::parse_str(&thread_context.locator.thread_id).with_context(|| {
            format!(
                "invalid thread_id `{}` on thread context",
                thread_context.locator.thread_id
            )
        })?;
        let mut state = self.state.lock().await;
        let actual_revision = state
            .threads
            .get(&thread_id)
            .map(Thread::revision)
            .unwrap_or_default();
        if !state.sessions.contains_key(&session_key) {
            return Err(anyhow!("session `{}` was not resolved before save", session_id).into());
        }
        let expected_revision = thread_context.revision();
        if expected_revision != actual_revision {
            return Err(SessionRevisionConflict {
                thread_id: thread_context.locator.thread_id.clone(),
                expected_revision,
                actual_revision,
            }
            .into());
        }

        let new_revision = actual_revision + 1;
        let mut stored = thread_context.clone();
        stored.detach_runtime();
        stored.set_revision(new_revision);
        state.threads.insert(thread_id, stored);
        let session = state
            .sessions
            .get_mut(&session_key)
            .expect("session should exist after presence check");
        session.updated_at = updated_at;
        if let Some(record) = dedup_record {
            state.external_messages.insert(
                (record.thread_id, record.external_message_id.clone()),
                record.clone(),
            );
        }

        info!(
            session_id = %session_id,
            thread_id = %thread_id,
            revision = new_revision,
            "saved thread context to memory session store"
        );
        Ok(new_revision)
    }

    async fn load_external_message_record(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<Option<ExternalMessageDedupRecord>> {
        let state = self.state.lock().await;
        Ok(state
            .external_messages
            .get(&(locator.thread_id, external_message_id.to_string()))
            .cloned())
    }

    async fn save_external_message_record(
        &self,
        locator: &ThreadLocator,
        record: &ExternalMessageDedupRecord,
    ) -> SessionStoreResult<()> {
        let mut state = self.state.lock().await;
        state.external_messages.insert(
            (locator.thread_id, record.external_message_id.clone()),
            record.clone(),
        );
        Ok(())
    }
}
