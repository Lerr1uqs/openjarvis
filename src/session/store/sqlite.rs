//! SQLite-backed `SessionStore` implementation for persisted thread-context recovery.

use super::{
    ExternalMessageDedupRecord, SessionRevisionConflict, SessionStore, SessionStoreError,
    SessionStoreResult, StoredSessionRecord,
};
use crate::{
    session::{SessionKey, ThreadLocator},
    thread::Thread,
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::info;
use uuid::Uuid;

const SQLITE_SCHEMA_VERSION: i64 = 2;

/// SQLite store backend that persists session metadata, thread snapshots, and dedup records.
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
            match current_version {
                0 => {
                    connection
                        .execute_batch(
                            r#"
CREATE TABLE IF NOT EXISTS session_metadata (
    session_id TEXT PRIMARY KEY,
    channel TEXT NOT NULL,
    user_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(channel, user_id)
);

CREATE TABLE IF NOT EXISTS thread_metadata (
    thread_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    external_thread_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES session_metadata(session_id) ON DELETE CASCADE,
    UNIQUE(session_id, external_thread_id)
);

CREATE TABLE IF NOT EXISTS external_message_dedup (
    thread_id TEXT NOT NULL,
    external_message_id TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    PRIMARY KEY(thread_id, external_message_id),
    FOREIGN KEY(thread_id) REFERENCES thread_metadata(thread_id) ON DELETE CASCADE
);

PRAGMA user_version = 2;
"#,
                        )
                        .context("failed to initialize sqlite session store schema")?;
                    info!(
                        sqlite_path = %sqlite_path.display(),
                        schema_version = SQLITE_SCHEMA_VERSION,
                        "initialized sqlite session store schema"
                    );
                    Ok(())
                }
                1 => {
                    connection
                        .execute_batch(
                            r#"
ALTER TABLE external_message_dedup RENAME TO external_message_dedup_v1;

CREATE TABLE external_message_dedup (
    thread_id TEXT NOT NULL,
    external_message_id TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    PRIMARY KEY(thread_id, external_message_id),
    FOREIGN KEY(thread_id) REFERENCES thread_metadata(thread_id) ON DELETE CASCADE
);

INSERT INTO external_message_dedup (thread_id, external_message_id, completed_at)
SELECT thread_id, external_message_id, completed_at
FROM external_message_dedup_v1;

DROP TABLE external_message_dedup_v1;

PRAGMA user_version = 2;
"#,
                        )
                        .context("failed to migrate sqlite session store schema to v2")?;
                    info!(
                        sqlite_path = %sqlite_path.display(),
                        schema_version = SQLITE_SCHEMA_VERSION,
                        "migrated sqlite session store schema to remove turn_id"
                    );
                    Ok(())
                }
                SQLITE_SCHEMA_VERSION => Ok(()),
                other => Err(anyhow!(
                    "unsupported sqlite session store schema version `{other}` at {}",
                    sqlite_path.display()
                )
                .into()),
            }
        })
        .await
        .map_err(|error| SessionStoreError::from(anyhow!("sqlite schema task failed: {error}")))?
    }

    async fn load_session(
        &self,
        key: &SessionKey,
    ) -> SessionStoreResult<Option<StoredSessionRecord>> {
        let key = key.clone();
        self.run_blocking(move |connection| {
            connection
                .query_row(
                    r#"
SELECT session_id, created_at, updated_at
FROM session_metadata
WHERE channel = ?1 AND user_id = ?2
"#,
                    params![&key.channel, &key.user_id],
                    |row| {
                        let session_id = row.get::<_, String>(0)?;
                        let created_at = row.get::<_, String>(1)?;
                        let updated_at = row.get::<_, String>(2)?;
                        Ok((session_id, created_at, updated_at))
                    },
                )
                .optional()
                .context("failed to load sqlite session metadata")?
                .map(|(session_id, created_at, updated_at)| {
                    Ok::<StoredSessionRecord, anyhow::Error>(StoredSessionRecord {
                        id: Uuid::parse_str(&session_id)
                            .with_context(|| format!("invalid stored session_id `{session_id}`"))?,
                        key,
                        created_at: DateTime::parse_from_rfc3339(&created_at)
                            .with_context(|| format!("invalid stored created_at `{created_at}`"))?
                            .with_timezone(&Utc),
                        updated_at: DateTime::parse_from_rfc3339(&updated_at)
                            .with_context(|| format!("invalid stored updated_at `{updated_at}`"))?
                            .with_timezone(&Utc),
                    })
                })
                .transpose()
                .map_err(Into::into)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!("sqlite load_session task failed: {error}"))
        })?
    }

    async fn resolve_or_create_session(
        &self,
        key: &SessionKey,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<StoredSessionRecord> {
        let key = key.clone();
        self.run_blocking(move |connection| {
            let tx = connection
                .transaction()
                .context("failed to start sqlite session resolve transaction")?;
            let existing = tx
                .query_row(
                    r#"
SELECT session_id, created_at, updated_at
FROM session_metadata
WHERE channel = ?1 AND user_id = ?2
"#,
                    params![&key.channel, &key.user_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .context("failed to query sqlite session metadata")?;
            let record = if let Some((session_id, created_at, _updated_at)) = existing {
                tx.execute(
                    "UPDATE session_metadata SET updated_at = ?1 WHERE session_id = ?2",
                    params![now.to_rfc3339(), session_id],
                )
                .context("failed to update sqlite session updated_at")?;
                StoredSessionRecord {
                    id: Uuid::parse_str(&session_id)
                        .with_context(|| format!("invalid stored session_id `{session_id}`"))?,
                    key: key.clone(),
                    created_at: DateTime::parse_from_rfc3339(&created_at)
                        .with_context(|| format!("invalid stored created_at `{created_at}`"))?
                        .with_timezone(&Utc),
                    updated_at: now,
                }
            } else {
                let session_id = Uuid::new_v4();
                tx.execute(
                    r#"
INSERT INTO session_metadata (session_id, channel, user_id, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5)
"#,
                    params![
                        session_id.to_string(),
                        &key.channel,
                        &key.user_id,
                        now.to_rfc3339(),
                        now.to_rfc3339()
                    ],
                )
                .context("failed to insert sqlite session metadata")?;
                StoredSessionRecord {
                    id: session_id,
                    key: key.clone(),
                    created_at: now,
                    updated_at: now,
                }
            };
            tx.commit()
                .context("failed to commit sqlite session resolve transaction")?;
            Ok(record)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!(
                "sqlite resolve_or_create_session task failed: {error}"
            ))
        })?
    }

    async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<Thread>> {
        let session_id = locator.session_id.to_string();
        let thread_id = locator.thread_id.to_string();
        let resolved_locator = crate::thread::ThreadContextLocator::from(locator);
        self.run_blocking(move |connection| {
            connection
                .query_row(
                    r#"
SELECT snapshot_json, revision
FROM thread_metadata
WHERE session_id = ?1 AND thread_id = ?2
"#,
                    params![session_id, thread_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .context("failed to load sqlite thread snapshot")?
                .map(|(snapshot_json, revision)| {
                    let mut thread_context = serde_json::from_str::<Thread>(&snapshot_json)
                        .with_context(|| "failed to deserialize sqlite thread snapshot")?;
                    thread_context.rebind_locator(resolved_locator);
                    thread_context.set_revision(
                        u64::try_from(revision).context("sqlite revision must not be negative")?,
                    );
                    Ok::<Thread, anyhow::Error>(thread_context)
                })
                .transpose()
                .map_err(Into::into)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!("sqlite load_thread_context task failed: {error}"))
        })?
    }

    async fn save_thread_context(
        &self,
        thread_context: &Thread,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<u64> {
        let thread_context = thread_context.clone();
        let dedup_record = dedup_record.cloned();
        let sqlite_path = self.path.clone();
        self.run_blocking(move |connection| {
            let session_id = thread_context
                .locator
                .session_id
                .as_deref()
                .context("thread context session_id is required before sqlite save")?;
            let tx = connection
                .transaction()
                .context("failed to start sqlite thread save transaction")?;
            let actual_revision = tx
                .query_row(
                    "SELECT revision FROM thread_metadata WHERE thread_id = ?1",
                    params![thread_context.locator.thread_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("failed to query current sqlite thread revision")?
                .unwrap_or_default();
            let expected_revision = i64::try_from(thread_context.revision())
                .context("thread revision exceeded sqlite integer range")?;
            if actual_revision != expected_revision {
                return Err(SessionRevisionConflict {
                    thread_id: thread_context.locator.thread_id.clone(),
                    expected_revision: thread_context.revision(),
                    actual_revision: u64::try_from(actual_revision).unwrap_or_default(),
                }
                .into());
            }

            let new_revision = actual_revision + 1;
            let snapshot_json = serde_json::to_string(&thread_context)
                .context("failed to serialize thread snapshot for sqlite save")?;
            tx.execute(
                "UPDATE session_metadata SET updated_at = ?1 WHERE session_id = ?2",
                params![updated_at.to_rfc3339(), session_id],
            )
            .context("failed to update sqlite session updated_at before thread save")?;

            if actual_revision == 0 {
                tx.execute(
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
                        thread_context.locator.thread_id,
                        session_id,
                        thread_context.locator.external_thread_id,
                        new_revision,
                        snapshot_json,
                        thread_context.created_at.to_rfc3339(),
                        updated_at.to_rfc3339(),
                    ],
                )
                .context("failed to insert sqlite thread snapshot")?;
            } else {
                tx.execute(
                    r#"
UPDATE thread_metadata
SET session_id = ?1,
    external_thread_id = ?2,
    revision = ?3,
    snapshot_json = ?4,
    updated_at = ?5
WHERE thread_id = ?6
"#,
                    params![
                        session_id,
                        thread_context.locator.external_thread_id,
                        new_revision,
                        snapshot_json,
                        updated_at.to_rfc3339(),
                        thread_context.locator.thread_id,
                    ],
                )
                .context("failed to update sqlite thread snapshot")?;
            }

            if let Some(record) = dedup_record {
                tx.execute(
                    r#"
INSERT INTO external_message_dedup (thread_id, external_message_id, completed_at)
VALUES (?1, ?2, ?3)
ON CONFLICT(thread_id, external_message_id)
DO UPDATE SET completed_at = excluded.completed_at
"#,
                    params![
                        record.thread_id.to_string(),
                        record.external_message_id,
                        record.completed_at.to_rfc3339(),
                    ],
                )
                .context("failed to upsert sqlite external_message_dedup from thread save")?;
            }

            tx.commit()
                .context("failed to commit sqlite thread save transaction")?;
            info!(
                sqlite_path = %sqlite_path.display(),
                thread_id = %thread_context.locator.thread_id,
                revision = new_revision,
                "saved thread context to sqlite session store"
            );
            u64::try_from(new_revision)
                .context("sqlite revision must not be negative")
                .map_err(Into::into)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!("sqlite save_thread_context task failed: {error}"))
        })?
    }

    async fn load_external_message_record(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<Option<ExternalMessageDedupRecord>> {
        let thread_id = locator.thread_id.to_string();
        let record_thread_id = locator.thread_id;
        let external_message_id = external_message_id.to_string();
        self.run_blocking(move |connection| {
            connection
                .query_row(
                    r#"
SELECT completed_at
FROM external_message_dedup
WHERE thread_id = ?1 AND external_message_id = ?2
"#,
                    params![thread_id, external_message_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("failed to load sqlite external message dedup record")?
                .map(|completed_at| {
                    Ok::<ExternalMessageDedupRecord, anyhow::Error>(ExternalMessageDedupRecord {
                        thread_id: record_thread_id,
                        external_message_id: external_message_id.clone(),
                        completed_at: DateTime::parse_from_rfc3339(&completed_at)
                            .with_context(|| {
                                format!("invalid stored dedup completed_at `{completed_at}`")
                            })?
                            .with_timezone(&Utc),
                    })
                })
                .transpose()
                .map_err(Into::into)
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!(
                "sqlite load_external_message_record task failed: {error}"
            ))
        })?
    }

    async fn save_external_message_record(
        &self,
        locator: &ThreadLocator,
        record: &ExternalMessageDedupRecord,
    ) -> SessionStoreResult<()> {
        let thread_id = locator.thread_id.to_string();
        let external_message_id = record.external_message_id.clone();
        let completed_at = record.completed_at.to_rfc3339();
        self.run_blocking(move |connection| {
            connection
                .execute(
                    r#"
INSERT INTO external_message_dedup (thread_id, external_message_id, completed_at)
VALUES (?1, ?2, ?3)
ON CONFLICT(thread_id, external_message_id)
DO UPDATE SET completed_at = excluded.completed_at
"#,
                    params![thread_id, external_message_id, completed_at],
                )
                .context("failed to upsert sqlite external_message_dedup record")?;
            Ok(())
        })
        .await
        .map_err(|error| {
            SessionStoreError::from(anyhow!(
                "sqlite save_external_message_record task failed: {error}"
            ))
        })?
    }
}
