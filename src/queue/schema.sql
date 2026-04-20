CREATE TABLE IF NOT EXISTS queue_schema_meta (
    schema_key TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS queue_message (
    message_id TEXT PRIMARY KEY,
    topic TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL,
    claim_token TEXT,
    leased_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_queue_message_pending_topic_created_at
    ON queue_message (topic, created_at, message_id)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_queue_message_active_topic_leased_until
    ON queue_message (topic, leased_until)
    WHERE status = 'active';

CREATE TABLE IF NOT EXISTS queue_worker (
    worker_id TEXT PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    lease_token TEXT NOT NULL,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    last_heartbeat_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    stopped_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_queue_worker_active_domain_expiry
    ON queue_worker (domain, lease_expires_at)
    WHERE stopped_at IS NULL;
