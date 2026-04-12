//! SQLite-backed thread-first `SessionStore` implementation.

use super::{
    SessionRevisionConflict, SessionStore, SessionStoreError, SessionStoreResult,
    StoredThreadRecord,
};
use crate::{
    session::ThreadLocator,
    thread::{PersistedThreadSnapshot, ThreadContextLocator, ThreadSnapshotStore},
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::info;

const SQLITE_SCHEMA_VERSION: i64 = 3;

/// SQLite store backend that persists thread snapshots and revisions.
pub struct SqliteSessionStore {
    path: PathBuf,
    connection: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SqliteSessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteSessionStore")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteSessionStore {
    /// Open one SQLite session store at the provided path.
    ///
    /// Parent directories are created automatically when they do not exist.
    pub async fn open(path: impl AsRef<Path>) -> SessionStoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create sqlite session store parent directory {}",
                    parent.display()
                )
            })?;
        }

        let opened_path = path.clone();
        let connection = tokio::task::spawn_blocking(move || -> SessionStoreResult<Connection> {
            let connection = Connection::open(&opened_path).with_context(|| {
                format!(
                    "failed to open sqlite session store at {}",
                    opened_path.display()
                )
            })?;
            connection
                .busy_timeout(Duration::from_secs(5))
                .context("failed to configure sqlite busy timeout")?;
            connection
                .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
                .context("failed to configure sqlite pragmas")?;
            Ok(connection)
        })
        .await
        .map_err(|error| SessionStoreError::from(anyhow!("sqlite open task failed: {error}")))??;

        Ok(Self {
            path,
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn run_blocking<T, F>(&self, operation: F) -> tokio::task::JoinHandle<SessionStoreResult<T>>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> SessionStoreResult<T> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut connection = connection
                .lock()
                .map_err(|_| anyhow!("sqlite session store connection lock poisoned"))?;
            operation(&mut connection)
        })
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn initialize_schema(&self) -> SessionStoreResult<()> {
        let sqlite_path = self.path.clone();
        self.run_blocking(move |connection| {
            let current_version = connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .context("failed to read sqlite user_version")?;

            if current_version != 0 && current_version != SQLITE_SCHEMA_VERSION {
                connection
                    .execute_batch(
                        r#"
DROP TABLE IF EXISTS thread_snapshots;
DROP TABLE IF EXISTS external_message_dedup;
DROP TABLE IF EXISTS thread_metadata;
DROP TABLE IF EXISTS session_metadata;
PRAGMA user_version = 0;
"#,
                    )
                    .context("failed to reset incompatible sqlite schema")?;
                info!(
                    sqlite_path = %sqlite_path.display(),
                    old_schema_version = current_version,
                    new_schema_version = SQLITE_SCHEMA_VERSION,
                    "reset incompatible sqlite session store schema to thread-first layout"
                );
            }

            connection
                .execute_batch(
                    r#"
CREATE TABLE IF NOT EXISTS thread_snapshots (
    thread_id TEXT PRIMARY KEY,
    thread_key TEXT NOT NULL UNIQUE,
    channel TEXT NOT NULL,
    user_id TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    session_id TEXT,
    revision INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

PRAGMA user_version = 3;
"#,
                )
                .context("failed to initialize sqlite session store schema")?;
            info!(
                sqlite_path = %sqlite_path.display(),
                schema_version = SQLITE_SCHEMA_VERSION,
                "initialized sqlite session store schema"
            );
            Ok(())
        })
        .await
        .map_err(|error| SessionStoreError::from(anyhow!("sqlite schema task failed: {error}")))?
    }

    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<StoredThreadRecord>> {
        let thread_id = locator.thread_id.to_string();
        self.run_blocking(move |connection| {
            connection
                .query_row(
                    r#"
SELECT session_id, channel, user_id, external_thread_id, snapshot_json, revision
FROM thread_snapshots
WHERE thread_id = ?1
"#,
                    params![thread_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()
                .context("failed to load sqlite thread snapshot")?
                .map(
                    |(
                        session_id,
                        channel,
                        user_id,
                        external_thread_id,
                        snapshot_json,
                        revision,
                    )| {
                        let locator = ThreadContextLocator::new(
                            session_id,
                            channel,
                            user_id,
                            external_thread_id,
                            thread_id,
                        );
                        let snapshot =
                            serde_json::from_str::<PersistedThreadSnapshot>(&snapshot_json)
                                .context("failed to deserialize sqlite thread snapshot")?;
                        Ok::<StoredThreadRecord, anyhow::Error>(StoredThreadRecord {
                            locator,
                            snapshot,
                            revision: u64::try_from(revision)
                                .context("sqlite revision must not be negative")?,
                        })
                    },
                )
                .transpose()
                .map_err(Into::into)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!("sqlite load_thread_context task failed: {error}"))
        })?
    }
}

#[async_trait]
impl ThreadSnapshotStore for SqliteSessionStore {
    async fn save_thread_snapshot(
        &self,
        locator: &ThreadContextLocator,
        snapshot: &PersistedThreadSnapshot,
        expected_revision: u64,
    ) -> anyhow::Result<u64> {
        let locator = locator.clone();
        let snapshot = snapshot.clone();
        let result = self
            .run_blocking(move |connection| {
                let tx = connection
                    .transaction()
                    .context("failed to start sqlite thread save transaction")?;
                let existing = tx
                    .query_row(
                        r#"
SELECT revision
FROM thread_snapshots
WHERE thread_id = ?1
"#,
                        params![&locator.thread_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .context("failed to query sqlite thread revision")?;
                let actual_revision = existing
                    .map(|revision| {
                        u64::try_from(revision).context("sqlite revision must not be negative")
                    })
                    .transpose()?
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
                let snapshot_json = serde_json::to_string(&snapshot)
                    .context("failed to serialize thread snapshot")?;
                let created_at = snapshot.thread.created_at.to_rfc3339();
                let updated_at = snapshot.thread.updated_at.to_rfc3339();
                tx.execute(
                    r#"
INSERT INTO thread_snapshots (
    thread_id,
    thread_key,
    channel,
    user_id,
    external_thread_id,
    session_id,
    revision,
    snapshot_json,
    created_at,
    updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(thread_id) DO UPDATE SET
    thread_key = excluded.thread_key,
    channel = excluded.channel,
    user_id = excluded.user_id,
    external_thread_id = excluded.external_thread_id,
    session_id = excluded.session_id,
    revision = excluded.revision,
    snapshot_json = excluded.snapshot_json,
    created_at = excluded.created_at,
    updated_at = excluded.updated_at
"#,
                    params![
                        &locator.thread_id,
                        locator.thread_key(),
                        &locator.channel,
                        &locator.user_id,
                        &locator.external_thread_id,
                        locator.session_id.as_deref(),
                        i64::try_from(new_revision)
                            .context("thread revision does not fit in sqlite i64")?,
                        snapshot_json,
                        created_at,
                        updated_at,
                    ],
                )
                .context("failed to upsert sqlite thread snapshot")?;
                tx.commit()
                    .context("failed to commit sqlite thread save transaction")?;

                info!(
                    thread_id = %locator.thread_id,
                    revision = new_revision,
                    "saved thread snapshot to sqlite session store"
                );
                Ok(new_revision)
            })
            .await
            .map_err(|error| anyhow!("sqlite save_thread_snapshot task failed: {error}"))?;

        result.map_err(anyhow::Error::from)
    }
}
