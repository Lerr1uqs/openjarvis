//! PostgreSQL-backed topic queue for durable inbound message delivery and domain worker leases.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration as StdDuration};
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls, Row};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{model::IncomingMessage, session::ThreadLocator};

const QUEUE_SCHEMA_VERSION: i32 = 1;
const QUEUE_SCHEMA_SQL: &str = include_str!("schema.sql");

type QueueOperationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Runtime knobs shared by router reconciliation and domain worker lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopicQueueRuntimeConfig {
    pub lease_ttl: StdDuration,
    pub heartbeat_interval: StdDuration,
    pub idle_timeout: StdDuration,
    pub reconcile_interval: StdDuration,
    pub pending_topic_scan_limit: usize,
}

impl Default for TopicQueueRuntimeConfig {
    fn default() -> Self {
        Self {
            lease_ttl: StdDuration::from_secs(30),
            heartbeat_interval: StdDuration::from_secs(10),
            idle_timeout: StdDuration::from_secs(5),
            reconcile_interval: StdDuration::from_secs(10),
            pending_topic_scan_limit: 128,
        }
    }
}

/// Durable queue payload persisted for one inbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicQueuePayload {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
}

impl TopicQueuePayload {
    /// Build one durable queue payload from a resolved locator and the original inbound message.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     model::{IncomingMessage, ReplyTarget},
    ///     queue::TopicQueuePayload,
    ///     session::{SessionKey, ThreadLocator},
    /// };
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_queue".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_queue".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: Some("chat_queue".to_string()),
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_queue".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    /// let session_key = SessionKey::from_incoming(&incoming);
    /// let locator = ThreadLocator::new(
    ///     session_key.derive_session_id(),
    ///     &incoming,
    ///     "chat_queue",
    ///     session_key.derive_thread_id("chat_queue"),
    /// );
    ///
    /// let payload = TopicQueuePayload::new(locator.clone(), incoming.clone());
    /// assert_eq!(payload.locator.thread_key(), locator.thread_key());
    /// assert_eq!(payload.incoming.content, "hello");
    /// ```
    pub fn new(locator: ThreadLocator, incoming: IncomingMessage) -> Self {
        Self { locator, incoming }
    }
}

/// Persisted queue message fact.
#[derive(Debug, Clone)]
pub struct TopicQueueMessage {
    pub message_id: Uuid,
    pub topic: String,
    pub payload: TopicQueuePayload,
    pub created_at: DateTime<Utc>,
}

/// One claimed queue message leased to a domain worker.
#[derive(Debug, Clone)]
pub struct ClaimedTopicQueueMessage {
    pub message_id: Uuid,
    pub topic: String,
    pub payload: TopicQueuePayload,
    pub claim_token: String,
    pub leased_until: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub claimed_at: DateTime<Utc>,
}

/// Active worker lease held for one domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicQueueWorkerLease {
    pub worker_id: String,
    pub domain: String,
    pub lease_token: String,
    pub lease_expires_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
}

/// Outcome of one domain lease acquisition attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicQueueLeaseAcquireResult {
    Acquired(TopicQueueWorkerLease),
    Busy,
}

/// Cleanup result for expired workers and stranded active messages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopicQueueReapReport {
    pub expired_domains: Vec<String>,
    pub recovered_message_ids: Vec<Uuid>,
}

/// Queue-facing abstraction used by router reconciliation and domain workers.
#[async_trait]
pub trait TopicQueue: Send + Sync {
    fn runtime_config(&self) -> TopicQueueRuntimeConfig;

    async fn initialize_schema(&self) -> Result<()>;
    async fn add(&self, topic: &str, payload: TopicQueuePayload) -> Result<TopicQueueMessage>;
    async fn claim(
        &self,
        domain: &str,
        lease: &TopicQueueWorkerLease,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedTopicQueueMessage>>;
    async fn complete(
        &self,
        message_id: Uuid,
        claim_token: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<bool>;
    async fn acquire_worker_lease(
        &self,
        domain: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<TopicQueueLeaseAcquireResult>;
    async fn heartbeat_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        now: DateTime<Utc>,
    ) -> Result<bool>;
    async fn release_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        released_at: DateTime<Utc>,
    ) -> Result<bool>;
    async fn reap_expired(&self, now: DateTime<Utc>) -> Result<TopicQueueReapReport>;
    async fn pending_topics(&self, limit: usize) -> Result<Vec<String>>;
    async fn is_worker_active(&self, domain: &str, now: DateTime<Utc>) -> Result<bool>;
}

/// PostgreSQL-backed durable topic queue.
pub struct PostgresTopicQueue {
    connection_label: String,
    runtime_config: TopicQueueRuntimeConfig,
    engine: Arc<PostgresQueueTransactionEngine>,
}

impl std::fmt::Debug for PostgresTopicQueue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresTopicQueue")
            .field("connection_label", &self.connection_label)
            .field("runtime_config", &self.runtime_config)
            .finish()
    }
}

impl PostgresTopicQueue {
    /// Connect one PostgreSQL topic queue from a database URL and runtime knobs.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::queue::{PostgresTopicQueue, TopicQueue, TopicQueueRuntimeConfig};
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let queue = PostgresTopicQueue::connect(
    ///     "postgres://postgres:postgres@127.0.0.1:5432/openjarvis",
    ///     TopicQueueRuntimeConfig::default(),
    /// )
    /// .await?;
    /// queue.initialize_schema().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(
        database_url: impl AsRef<str>,
        runtime_config: TopicQueueRuntimeConfig,
    ) -> Result<Self> {
        let database_url = database_url.as_ref().trim();
        if database_url.is_empty() {
            bail!("queue.database_url must not be blank when PostgreSQL topic queue is enabled");
        }

        let (client, connection) = tokio_postgres::connect(database_url, NoTls)
            .await
            .context("failed to connect postgresql topic queue")?;
        let connection_label = sanitize_database_url(database_url);
        let queue_connection_label = connection_label.clone();
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                warn!(
                    queue_database = %queue_connection_label,
                    error = %error,
                    "postgresql topic queue connection task exited"
                );
            }
        });

        info!(
            queue_database = %connection_label,
            lease_ttl_secs = runtime_config.lease_ttl.as_secs(),
            heartbeat_interval_secs = runtime_config.heartbeat_interval.as_secs(),
            idle_timeout_secs = runtime_config.idle_timeout.as_secs(),
            reconcile_interval_secs = runtime_config.reconcile_interval.as_secs(),
            pending_topic_scan_limit = runtime_config.pending_topic_scan_limit,
            "connected postgresql topic queue"
        );

        Ok(Self {
            connection_label,
            runtime_config,
            engine: Arc::new(PostgresQueueTransactionEngine::new(client)),
        })
    }

    fn lease_expiration_at(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
        now.checked_add_signed(duration_to_chrono(self.runtime_config.lease_ttl)?)
            .context("queue lease expiry overflowed chrono DateTime")
    }
}

#[async_trait]
impl TopicQueue for PostgresTopicQueue {
    fn runtime_config(&self) -> TopicQueueRuntimeConfig {
        self.runtime_config
    }

    async fn initialize_schema(&self) -> Result<()> {
        let queue_database = self.connection_label.clone();
        self.engine
            .run("initialize_schema", move |client| {
                Box::pin(async move {
                    client
                        .batch_execute(QUEUE_SCHEMA_SQL)
                        .await
                        .context("failed to initialize postgresql topic queue tables")?;
                    client
                        .execute(
                            r#"
INSERT INTO queue_schema_meta (schema_key, schema_version, updated_at)
VALUES ('postgresql_topic_queue', $1, $2)
ON CONFLICT(schema_key) DO UPDATE SET
    schema_version = EXCLUDED.schema_version,
    updated_at = EXCLUDED.updated_at
"#,
                            &[&QUEUE_SCHEMA_VERSION, &Utc::now()],
                        )
                        .await
                        .context("failed to write postgresql topic queue schema meta")?;
                    info!(
                        queue_database = %queue_database,
                        schema_version = QUEUE_SCHEMA_VERSION,
                        "initialized postgresql topic queue schema"
                    );
                    Ok(())
                })
            })
            .await
    }

    async fn add(&self, topic: &str, payload: TopicQueuePayload) -> Result<TopicQueueMessage> {
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            bail!("queue topic must not be blank");
        }
        let message_id = Uuid::new_v4();
        let created_at = Utc::now();
        let payload_json =
            serde_json::to_value(&payload).context("failed to serialize topic queue payload")?;
        let queue_database = self.connection_label.clone();
        self.engine
            .run("add_message", move |client| {
                let topic = topic.clone();
                let payload_json = payload_json.clone();
                Box::pin(async move {
                    client
                        .execute(
                            r#"
INSERT INTO queue_message (
    message_id,
    topic,
    payload_json,
    status,
    claim_token,
    leased_until,
    created_at,
    claimed_at,
    completed_at
)
VALUES ($1, $2, $3, 'pending', NULL, NULL, $4, NULL, NULL)
"#,
                            &[&message_id.to_string(), &topic, &payload_json, &created_at],
                        )
                        .await
                        .context("failed to insert topic queue message")?;
                    info!(
                        queue_database = %queue_database,
                        topic = %topic,
                        message_id = %message_id,
                        "enqueued topic queue message"
                    );
                    Ok(TopicQueueMessage {
                        message_id,
                        topic,
                        payload,
                        created_at,
                    })
                })
            })
            .await
    }

    async fn claim(
        &self,
        domain: &str,
        lease: &TopicQueueWorkerLease,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedTopicQueueMessage>> {
        let domain = domain.trim().to_string();
        let expected_lease = lease.clone();
        let leased_until = self.lease_expiration_at(now)?;
        let queue_database = self.connection_label.clone();
        self.engine
            .run("claim_message", move |client| {
                let domain = domain.clone();
                let expected_lease = expected_lease.clone();
                Box::pin(async move {
                    let worker_rows = client
                        .query(
                            r#"
SELECT lease_expires_at, stopped_at
FROM queue_worker
WHERE domain = $1 AND worker_id = $2 AND lease_token = $3
FOR UPDATE
"#,
                            &[
                                &domain,
                                &expected_lease.worker_id,
                                &expected_lease.lease_token,
                            ],
                        )
                        .await
                        .context("failed to lock topic queue worker lease before claim")?;
                    let Some(worker_row) = worker_rows.into_iter().next() else {
                        debug!(
                            queue_database = %queue_database,
                            domain = %domain,
                            worker_id = %expected_lease.worker_id,
                            "skip claim because worker lease is no longer active"
                        );
                        return Ok(None);
                    };
                    let worker_stopped_at: Option<DateTime<Utc>> = worker_row.get(1);
                    let worker_lease_expires_at: DateTime<Utc> = worker_row.get(0);
                    if worker_stopped_at.is_some() || worker_lease_expires_at <= now {
                        debug!(
                            queue_database = %queue_database,
                            domain = %domain,
                            worker_id = %expected_lease.worker_id,
                            worker_lease_expires_at = %worker_lease_expires_at,
                            "skip claim because worker lease already expired"
                        );
                        return Ok(None);
                    }

                    let rows = client
                        .query(
                            r#"
WITH next_message AS (
    SELECT message_id
    FROM queue_message
    WHERE topic = $1 AND status = 'pending'
    ORDER BY created_at ASC, message_id ASC
    LIMIT 1
    FOR UPDATE
)
UPDATE queue_message
SET status = 'active',
    claim_token = $2,
    leased_until = $3,
    claimed_at = $4,
    completed_at = NULL
WHERE message_id = (SELECT message_id FROM next_message)
RETURNING message_id, topic, payload_json, created_at, claimed_at, leased_until
"#,
                            &[&domain, &expected_lease.lease_token, &leased_until, &now],
                        )
                        .await
                        .context("failed to claim next topic queue message")?;
                    let Some(row) = rows.into_iter().next() else {
                        return Ok(None);
                    };
                    let claimed = decode_claimed_message_row(row, &expected_lease.lease_token)
                        .context("failed to decode claimed topic queue message")?;
                    info!(
                        queue_database = %queue_database,
                        domain = %domain,
                        worker_id = %expected_lease.worker_id,
                        message_id = %claimed.message_id,
                        leased_until = %claimed.leased_until,
                        "claimed topic queue message"
                    );
                    Ok(Some(claimed))
                })
            })
            .await
    }

    async fn complete(
        &self,
        message_id: Uuid,
        claim_token: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<bool> {
        let claim_token = claim_token.to_string();
        let queue_database = self.connection_label.clone();
        self.engine
            .run("complete_message", move |client| {
                let claim_token = claim_token.clone();
                Box::pin(async move {
                    let affected = client
                        .execute(
                            r#"
UPDATE queue_message
SET status = 'complete',
    claim_token = NULL,
    leased_until = NULL,
    completed_at = $3
WHERE message_id = $1 AND claim_token = $2 AND status = 'active'
"#,
                            &[&message_id.to_string(), &claim_token, &completed_at],
                        )
                        .await
                        .context("failed to complete topic queue message")?;
                    if affected > 0 {
                        info!(
                            queue_database = %queue_database,
                            message_id = %message_id,
                            "completed topic queue message"
                        );
                    } else {
                        warn!(
                            queue_database = %queue_database,
                            message_id = %message_id,
                            "topic queue complete skipped because claim no longer matched"
                        );
                    }
                    Ok(affected > 0)
                })
            })
            .await
    }

    async fn acquire_worker_lease(
        &self,
        domain: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<TopicQueueLeaseAcquireResult> {
        let domain = domain.trim().to_string();
        if domain.is_empty() {
            bail!("queue worker domain must not be blank");
        }
        let worker_id = worker_id.trim().to_string();
        if worker_id.is_empty() {
            bail!("queue worker_id must not be blank");
        }
        let lease_token = Uuid::new_v4().to_string();
        let lease_expires_at = self.lease_expiration_at(now)?;
        let queue_database = self.connection_label.clone();
        self.engine
            .run("acquire_worker_lease", move |client| {
                let domain = domain.clone();
                let worker_id = worker_id.clone();
                let lease_token = lease_token.clone();
                Box::pin(async move {
                    let rows = client
                        .query(
                            r#"
SELECT worker_id, lease_token, lease_expires_at, stopped_at, started_at
FROM queue_worker
WHERE domain = $1
FOR UPDATE
"#,
                            &[&domain],
                        )
                        .await
                        .context("failed to lock topic queue worker row")?;
                    if let Some(row) = rows.into_iter().next() {
                        let current_stopped_at: Option<DateTime<Utc>> = row.get(3);
                        let current_lease_expires_at: DateTime<Utc> = row.get(2);
                        if current_stopped_at.is_none() && current_lease_expires_at > now {
                            debug!(
                                queue_database = %queue_database,
                                domain = %domain,
                                active_worker_id = %row.get::<_, String>(0),
                                lease_expires_at = %current_lease_expires_at,
                                "skip worker spawn because domain lease is already active"
                            );
                            return Ok(TopicQueueLeaseAcquireResult::Busy);
                        }

                        client
                            .execute(
                                r#"
UPDATE queue_worker
SET worker_id = $2,
    lease_token = $3,
    lease_expires_at = $4,
    last_heartbeat_at = $5,
    started_at = $5,
    stopped_at = NULL
WHERE domain = $1
"#,
                                &[&domain, &worker_id, &lease_token, &lease_expires_at, &now],
                            )
                            .await
                            .context("failed to update expired topic queue worker lease")?;
                    } else {
                        client
                            .execute(
                                r#"
INSERT INTO queue_worker (
    worker_id,
    domain,
    lease_token,
    lease_expires_at,
    last_heartbeat_at,
    started_at,
    stopped_at
)
VALUES ($1, $2, $3, $4, $5, $5, NULL)
"#,
                                &[&worker_id, &domain, &lease_token, &lease_expires_at, &now],
                            )
                            .await
                            .context("failed to insert topic queue worker lease")?;
                    }

                    let lease = TopicQueueWorkerLease {
                        worker_id,
                        domain,
                        lease_token,
                        lease_expires_at,
                        started_at: now,
                    };
                    info!(
                        queue_database = %queue_database,
                        domain = %lease.domain,
                        worker_id = %lease.worker_id,
                        lease_expires_at = %lease.lease_expires_at,
                        "acquired topic queue worker lease"
                    );
                    Ok(TopicQueueLeaseAcquireResult::Acquired(lease))
                })
            })
            .await
    }

    async fn heartbeat_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let lease = lease.clone();
        let lease_expires_at = self.lease_expiration_at(now)?;
        let queue_database = self.connection_label.clone();
        self.engine
            .run("heartbeat_worker", move |client| {
                let lease = lease.clone();
                Box::pin(async move {
                    let worker_updated = client
                        .execute(
                            r#"
UPDATE queue_worker
SET lease_expires_at = $4,
    last_heartbeat_at = $5
WHERE domain = $1
  AND worker_id = $2
  AND lease_token = $3
  AND stopped_at IS NULL
"#,
                            &[
                                &lease.domain,
                                &lease.worker_id,
                                &lease.lease_token,
                                &lease_expires_at,
                                &now,
                            ],
                        )
                        .await
                        .context("failed to heartbeat topic queue worker")?;
                    if worker_updated == 0 {
                        warn!(
                            queue_database = %queue_database,
                            domain = %lease.domain,
                            worker_id = %lease.worker_id,
                            "topic queue worker heartbeat skipped because lease is no longer active"
                        );
                        return Ok(false);
                    }

                    client
                        .execute(
                            r#"
UPDATE queue_message
SET leased_until = $2
WHERE topic = $1
  AND status = 'active'
  AND claim_token = $3
"#,
                            &[&lease.domain, &lease_expires_at, &lease.lease_token],
                        )
                        .await
                        .context(
                            "failed to extend active topic queue message lease during heartbeat",
                        )?;
                    debug!(
                        queue_database = %queue_database,
                        domain = %lease.domain,
                        worker_id = %lease.worker_id,
                        lease_expires_at = %lease_expires_at,
                        "heartbeat refreshed topic queue worker lease"
                    );
                    Ok(true)
                })
            })
            .await
    }

    async fn release_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        released_at: DateTime<Utc>,
    ) -> Result<bool> {
        let lease = lease.clone();
        let queue_database = self.connection_label.clone();
        self.engine
            .run("release_worker", move |client| {
                let lease = lease.clone();
                Box::pin(async move {
                    let affected = client
                        .execute(
                            r#"
UPDATE queue_worker
SET stopped_at = $4,
    lease_expires_at = $4,
    last_heartbeat_at = $4
WHERE domain = $1
  AND worker_id = $2
  AND lease_token = $3
  AND stopped_at IS NULL
"#,
                            &[
                                &lease.domain,
                                &lease.worker_id,
                                &lease.lease_token,
                                &released_at,
                            ],
                        )
                        .await
                        .context("failed to release topic queue worker lease")?;
                    if affected > 0 {
                        info!(
                            queue_database = %queue_database,
                            domain = %lease.domain,
                            worker_id = %lease.worker_id,
                            "released topic queue worker lease"
                        );
                    }
                    Ok(affected > 0)
                })
            })
            .await
    }

    async fn reap_expired(&self, now: DateTime<Utc>) -> Result<TopicQueueReapReport> {
        let queue_database = self.connection_label.clone();
        self.engine
            .run("reap_expired", move |client| {
                Box::pin(async move {
                    let expired_rows = client
                        .query(
                            r#"
SELECT domain
FROM queue_worker
WHERE stopped_at IS NULL AND lease_expires_at <= $1
FOR UPDATE
"#,
                            &[&now],
                        )
                        .await
                        .context("failed to query expired topic queue workers")?;
                    let expired_domains = expired_rows
                        .iter()
                        .map(|row| row.get::<_, String>(0))
                        .collect::<Vec<_>>();

                    if !expired_domains.is_empty() {
                        client
                            .execute(
                                r#"
UPDATE queue_worker
SET stopped_at = $2,
    last_heartbeat_at = $2,
    lease_expires_at = $2
WHERE domain = ANY($1) AND stopped_at IS NULL
"#,
                                &[&expired_domains, &now],
                            )
                            .await
                            .context("failed to stop expired topic queue workers")?;
                    }

                    // Recover by message lease instead of expired domain only. A new worker can
                    // reacquire the same domain before reap runs, but the old active message still
                    // belongs to the previous lease token and must remain recoverable.
                    let recovered_rows = client
                        .query(
                            r#"
UPDATE queue_message
SET status = 'pending',
    claim_token = NULL,
    leased_until = NULL,
    claimed_at = NULL
WHERE status = 'active'
  AND leased_until IS NOT NULL
  AND leased_until <= $1
RETURNING message_id
"#,
                            &[&now],
                        )
                        .await
                        .context("failed to recover stranded active topic queue messages")?;
                    let recovered_message_ids = recovered_rows
                        .into_iter()
                        .map(|row| {
                            Uuid::parse_str(row.get::<_, String>(0).as_str())
                                .context("failed to parse recovered queue message id")
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if expired_domains.is_empty() && recovered_message_ids.is_empty() {
                        return Ok(TopicQueueReapReport::default());
                    }
                    warn!(
                        queue_database = %queue_database,
                        expired_domains = ?expired_domains,
                        recovered_message_ids = ?recovered_message_ids,
                        "reaped expired topic queue workers and recovered stranded messages; delivery semantics remain at-least-once"
                    );
                    Ok(TopicQueueReapReport {
                        expired_domains,
                        recovered_message_ids,
                    })
                })
            })
            .await
    }

    async fn pending_topics(&self, limit: usize) -> Result<Vec<String>> {
        let limit =
            i64::try_from(limit).context("pending topic scan limit does not fit into i64")?;
        let topics: Vec<String> = self
            .engine
            .run("pending_topics", move |client| {
                Box::pin(async move {
                    let rows = client
                        .query(
                            r#"
SELECT topic
FROM queue_message
WHERE status = 'pending'
GROUP BY topic
ORDER BY MIN(created_at) ASC, topic ASC
LIMIT $1
"#,
                            &[&limit],
                        )
                        .await
                        .context("failed to query pending topic queue domains")?;
                    Ok(rows
                        .into_iter()
                        .map(|row| row.get::<_, String>(0))
                        .collect())
                })
            })
            .await?;
        if !topics.is_empty() {
            debug!(pending_topics = ?topics, "scanned pending topic queue domains");
        }
        Ok(topics)
    }

    async fn is_worker_active(&self, domain: &str, now: DateTime<Utc>) -> Result<bool> {
        let domain = domain.trim().to_string();
        self.engine
            .run("is_worker_active", move |client| {
                let domain = domain.clone();
                Box::pin(async move {
                    let rows = client
                        .query(
                            r#"
SELECT 1
FROM queue_worker
WHERE domain = $1
  AND stopped_at IS NULL
  AND lease_expires_at > $2
LIMIT 1
"#,
                            &[&domain, &now],
                        )
                        .await
                        .context("failed to query active topic queue worker state")?;
                    Ok(!rows.is_empty())
                })
            })
            .await
    }
}

struct PostgresQueueTransactionEngine {
    client: Arc<Mutex<Client>>,
}

impl PostgresQueueTransactionEngine {
    fn new(client: Client) -> Self {
        Self {
            client: Arc::new(Mutex::new(client)),
        }
    }

    async fn run<T, F>(&self, operation_name: &'static str, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a Client) -> QueueOperationFuture<'a, T> + Send + 'static,
    {
        let client = self.client.lock().await;
        client
            .batch_execute("BEGIN")
            .await
            .with_context(|| format!("failed to begin queue transaction for `{operation_name}`"))?;
        let result = operation(&client).await;
        match result {
            Ok(value) => {
                client.batch_execute("COMMIT").await.with_context(|| {
                    format!("failed to commit queue transaction for `{operation_name}`")
                })?;
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback_error) = client.batch_execute("ROLLBACK").await {
                    warn!(
                        operation_name,
                        error = %rollback_error,
                        "failed to rollback queue transaction after error"
                    );
                }
                Err(error)
            }
        }
    }
}

fn decode_claimed_message_row(row: Row, claim_token: &str) -> Result<ClaimedTopicQueueMessage> {
    let message_id_raw: String = row.get(0);
    let payload_json: Value = row.get(2);
    let payload = serde_json::from_value::<TopicQueuePayload>(payload_json)
        .context("failed to deserialize claimed topic queue payload")?;
    Ok(ClaimedTopicQueueMessage {
        message_id: Uuid::parse_str(&message_id_raw)
            .context("failed to parse claimed topic queue message id")?,
        topic: row.get(1),
        payload,
        claim_token: claim_token.to_string(),
        created_at: row.get(3),
        claimed_at: row.get(4),
        leased_until: row.get(5),
    })
}

fn duration_to_chrono(value: StdDuration) -> Result<Duration> {
    Duration::from_std(value).context("std::time::Duration does not fit in chrono::Duration")
}

fn sanitize_database_url(database_url: &str) -> String {
    database_url
        .split('@')
        .last()
        .map(|tail| format!("postgres://***@{tail}"))
        .unwrap_or_else(|| "postgres://***".to_string())
}
