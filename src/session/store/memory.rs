//! In-memory thread-first `SessionStore` implementation used by tests and non-persistent runtimes.

use super::{SessionRevisionConflict, SessionStore, SessionStoreResult, StoredThreadRecord};
use crate::{
    session::ThreadLocator,
    thread::{PersistedThreadSnapshot, ThreadContextLocator, ThreadSnapshotStore},
};
use anyhow::Context;
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::Mutex;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct StoredMemoryThread {
    locator: ThreadContextLocator,
    snapshot: PersistedThreadSnapshot,
    revision: u64,
}

#[derive(Debug, Default)]
struct MemoryStoreState {
    threads: HashMap<Uuid, StoredMemoryThread>,
}

/// In-memory store backend that keeps the same thread-first persistence contract as SQLite.
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
impl ThreadSnapshotStore for MemorySessionStore {
    async fn save_thread_snapshot(
        &self,
        locator: &ThreadContextLocator,
        snapshot: &PersistedThreadSnapshot,
        expected_revision: u64,
    ) -> anyhow::Result<u64> {
        let thread_id = Uuid::parse_str(&locator.thread_id).with_context(|| {
            format!(
                "invalid thread_id `{}` on thread locator",
                locator.thread_id
            )
        })?;
        let mut state = self.state.lock().await;
        let actual_revision = state
            .threads
            .get(&thread_id)
            .map(|record| record.revision)
            .unwrap_or_default();
        if expected_revision != actual_revision {
            return Err(SessionRevisionConflict {
                thread_id: locator.thread_id.clone(),
                expected_revision,
                actual_revision,
            }
            .into());
        }

        let new_revision = actual_revision + 1;
        state.threads.insert(
            thread_id,
            StoredMemoryThread {
                locator: locator.clone(),
                snapshot: snapshot.clone(),
                revision: new_revision,
            },
        );
        info!(
            thread_id = %locator.thread_id,
            revision = new_revision,
            "saved thread snapshot to memory session store"
        );
        Ok(new_revision)
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn initialize_schema(&self) -> SessionStoreResult<()> {
        Ok(())
    }

    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<StoredThreadRecord>> {
        let state = self.state.lock().await;
        Ok(state
            .threads
            .get(&locator.thread_id)
            .cloned()
            .map(|record| StoredThreadRecord {
                locator: record.locator,
                snapshot: record.snapshot,
                revision: record.revision,
            }))
    }

    async fn remove_thread_context(&self, locator: &ThreadLocator) -> SessionStoreResult<bool> {
        let mut state = self.state.lock().await;
        Ok(state.threads.remove(&locator.thread_id).is_some())
    }

    async fn list_child_threads(
        &self,
        parent_locator: &ThreadLocator,
    ) -> SessionStoreResult<Vec<StoredThreadRecord>> {
        let state = self.state.lock().await;
        let mut records = state
            .threads
            .values()
            .filter(|record| {
                record
                    .snapshot
                    .state
                    .child_thread
                    .as_ref()
                    .map(|child| child.parent_thread_id == parent_locator.thread_id.to_string())
                    .unwrap_or(false)
            })
            .cloned()
            .map(|record| StoredThreadRecord {
                locator: record.locator,
                snapshot: record.snapshot,
                revision: record.revision,
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.locator.thread_id.cmp(&right.locator.thread_id));
        Ok(records)
    }
}
