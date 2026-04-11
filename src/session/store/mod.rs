//! Pluggable thread-context persistence backends for `SessionManager`.
//!
//! The store boundary keeps session/thread persistence semantics stable while allowing the
//! runtime to swap between memory and SQLite implementations without changing router or agent
//! flows.

mod memory;
mod sqlite;

use crate::{
    session::{SessionKey, ThreadLocator},
    thread::Thread,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

pub use memory::MemorySessionStore;
pub use sqlite::SqliteSessionStore;

/// One persisted session metadata record resolved from the store backend.
#[derive(Debug, Clone)]
pub struct StoredSessionRecord {
    pub id: Uuid,
    pub key: SessionKey,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One persisted external-message deduplication record scoped to a single thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalMessageDedupRecord {
    pub thread_id: Uuid,
    pub external_message_id: String,
    pub completed_at: DateTime<Utc>,
}

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

/// Persistence boundary for session metadata, thread snapshots, and external-message deduplication.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Initialize backend schema or apply migrations before the store is used.
    async fn initialize_schema(&self) -> SessionStoreResult<()>;

    /// Load an existing session metadata record by its stable session key.
    async fn load_session(
        &self,
        key: &SessionKey,
    ) -> SessionStoreResult<Option<StoredSessionRecord>>;

    /// Resolve one session metadata record and create it on first use.
    async fn resolve_or_create_session(
        &self,
        key: &SessionKey,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<StoredSessionRecord>;

    /// Load the latest persisted thread snapshot for one resolved thread locator.
    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<Thread>>;

    /// Persist the latest thread snapshot with compare-and-swap revision semantics.
    async fn save_thread_context(
        &self,
        thread_context: &Thread,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<u64>;

    /// Load a thread-scoped external-message deduplication record.
    async fn load_external_message_record(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<Option<ExternalMessageDedupRecord>>;

    /// Persist a thread-scoped external-message deduplication record.
    async fn save_external_message_record(
        &self,
        locator: &ThreadLocator,
        record: &ExternalMessageDedupRecord,
    ) -> SessionStoreResult<()>;
}
