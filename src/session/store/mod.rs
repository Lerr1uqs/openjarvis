//! Thread-first persistence backends for `SessionManager`.
//!
//! The store boundary persists only thread identity, persisted thread snapshot, and revision.

mod memory;
mod sqlite;

use crate::{
    session::ThreadLocator,
    thread::{PersistedThreadSnapshot, ThreadContextLocator, ThreadSnapshotStore},
};
use async_trait::async_trait;
use thiserror::Error;

pub use memory::MemorySessionStore;
pub use sqlite::SqliteSessionStore;

/// One compare-and-swap write conflict returned when an older thread snapshot tries to overwrite
/// a newer persisted revision.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "thread `{thread_id}` revision conflict: expected revision {expected_revision}, actual revision {actual_revision}"
)]
pub struct SessionRevisionConflict {
    pub thread_id: String,
    pub expected_revision: u64,
    pub actual_revision: u64,
}

/// Unified error returned by session store backends.
#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error(transparent)]
    RevisionConflict(#[from] SessionRevisionConflict),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Shared result type for session store operations.
pub type SessionStoreResult<T> = Result<T, SessionStoreError>;

/// One persisted thread record resolved from the store backend.
#[derive(Debug, Clone)]
pub struct StoredThreadRecord {
    pub locator: ThreadContextLocator,
    pub snapshot: PersistedThreadSnapshot,
    pub revision: u64,
}

/// Persistence boundary for thread snapshots.
#[async_trait]
pub trait SessionStore: ThreadSnapshotStore + Send + Sync {
    /// Initialize backend schema or apply migrations before the store is used.
    async fn initialize_schema(&self) -> SessionStoreResult<()>;

    /// Load the latest persisted thread snapshot for one resolved thread locator.
    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<StoredThreadRecord>>;

    /// Remove one persisted thread snapshot by resolved thread locator.
    async fn remove_thread_context(&self, locator: &ThreadLocator) -> SessionStoreResult<bool>;

    /// List all persisted child threads owned by one parent thread.
    async fn list_child_threads(
        &self,
        parent_locator: &ThreadLocator,
    ) -> SessionStoreResult<Vec<StoredThreadRecord>>;
}
