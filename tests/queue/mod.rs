#[path = "../support/mod.rs"]
mod support;

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use openjarvis::{
    model::{IncomingMessage, ReplyTarget},
    queue::{
        PostgresTopicQueue, TopicQueue, TopicQueueLeaseAcquireResult, TopicQueuePayload,
        TopicQueueRuntimeConfig,
    },
    session::{SessionKey, ThreadLocator},
};
use serde_json::json;
use std::{env, time::Duration};
use support::TestTopicQueue;
use tokio_postgres::NoTls;
use uuid::Uuid;

fn build_incoming(message_id: &str, content: &str, external_thread_id: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_queue".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some(external_thread_id.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_queue".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_locator(incoming: &IncomingMessage) -> ThreadLocator {
    let session_key = SessionKey::from_incoming(incoming);
    let external_thread_id = incoming.resolved_external_thread_id();
    ThreadLocator::new(
        session_key.derive_session_id(),
        incoming,
        external_thread_id.clone(),
        session_key.derive_thread_id(&external_thread_id),
    )
}

fn test_runtime_config() -> TopicQueueRuntimeConfig {
    TopicQueueRuntimeConfig {
        lease_ttl: Duration::from_millis(100),
        heartbeat_interval: Duration::from_millis(30),
        idle_timeout: Duration::from_millis(50),
        reconcile_interval: Duration::from_millis(50),
        pending_topic_scan_limit: 16,
    }
}

fn postgres_test_database_url() -> Option<String> {
    env::var("OPENJARVIS_TEST_POSTGRES_URL")
        .ok()
        .map(|database_url| database_url.trim().to_string())
        .filter(|database_url| !database_url.is_empty())
}

async fn reset_postgres_queue_tables(database_url: &str) -> Result<()> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls).await?;
    let connection_task = tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(
            r#"
DROP TABLE IF EXISTS queue_message;
DROP TABLE IF EXISTS queue_worker;
DROP TABLE IF EXISTS queue_schema_meta;
"#,
        )
        .await?;
    drop(client);
    connection_task.abort();
    let _ = connection_task.await;
    Ok(())
}

#[tokio::test]
async fn queue_add_claim_complete_flows_in_created_order() -> Result<()> {
    // 测试场景: queue 必须按 created_at 顺序 claim pending message，并在 complete 后留下可观测完成状态。
    let queue = TestTopicQueue::default();
    let first = build_incoming("msg_queue_1", "first", "chat_queue");
    let second = build_incoming("msg_queue_2", "second", "chat_queue");
    let first_locator = build_locator(&first);
    let second_locator = build_locator(&second);

    queue
        .add(
            &first_locator.thread_key(),
            TopicQueuePayload::new(first_locator.clone(), first.clone()),
        )
        .await?;
    queue
        .add(
            &second_locator.thread_key(),
            TopicQueuePayload::new(second_locator.clone(), second.clone()),
        )
        .await?;

    let lease = match queue
        .acquire_worker_lease(&first_locator.thread_key(), "worker-a", Utc::now())
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("first lease should acquire"),
    };

    let claimed_first = queue
        .claim(&first_locator.thread_key(), &lease, Utc::now())
        .await?
        .expect("first message should claim");
    assert_eq!(claimed_first.payload.incoming.content, "first");
    assert!(
        queue
            .complete(
                claimed_first.message_id,
                &claimed_first.claim_token,
                Utc::now()
            )
            .await?
    );

    let claimed_second = queue
        .claim(&second_locator.thread_key(), &lease, Utc::now())
        .await?
        .expect("second message should claim");
    assert_eq!(claimed_second.payload.incoming.content, "second");
    assert!(
        queue
            .complete(
                claimed_second.message_id,
                &claimed_second.claim_token,
                Utc::now()
            )
            .await?
    );

    let snapshot = queue.snapshot_messages().await;
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot.iter().all(|message| message.status == "complete"));
    Ok(())
}

#[tokio::test]
async fn queue_allows_only_one_active_worker_per_domain() -> Result<()> {
    // 测试场景: 同一 domain 同时只能有一个活跃 lease，直到旧 lease 释放或过期。
    let queue = TestTopicQueue::default();
    let now = Utc::now();
    let acquired = queue
        .acquire_worker_lease("ou_queue:feishu:chat_queue", "worker-a", now)
        .await?;
    let TopicQueueLeaseAcquireResult::Acquired(lease) = acquired else {
        panic!("first lease should acquire");
    };
    assert!(queue.is_worker_active(&lease.domain, now).await?);

    let busy = queue
        .acquire_worker_lease(&lease.domain, "worker-b", now + ChronoDuration::seconds(1))
        .await?;
    assert!(matches!(busy, TopicQueueLeaseAcquireResult::Busy));

    assert!(
        queue
            .release_worker(&lease, now + ChronoDuration::seconds(1))
            .await?
    );
    let reacquired = queue
        .acquire_worker_lease(&lease.domain, "worker-c", now + ChronoDuration::seconds(2))
        .await?;
    assert!(matches!(
        reacquired,
        TopicQueueLeaseAcquireResult::Acquired(_)
    ));
    Ok(())
}

#[tokio::test]
async fn queue_reap_recovers_expired_worker_and_stranded_message() -> Result<()> {
    // 测试场景: 过期 worker 必须被回收，stranded active message 必须恢复成 pending 以便后续重试。
    let queue = TestTopicQueue::new(test_runtime_config());
    let incoming = build_incoming("msg_queue_reap", "recover-me", "chat_queue_reap");
    let locator = build_locator(&incoming);
    queue
        .add(
            &locator.thread_key(),
            TopicQueuePayload::new(locator.clone(), incoming.clone()),
        )
        .await?;
    let lease = match queue
        .acquire_worker_lease(&locator.thread_key(), "worker-a", Utc::now())
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("lease should acquire"),
    };
    let claimed = queue
        .claim(&locator.thread_key(), &lease, Utc::now())
        .await?
        .expect("message should claim");

    let reap_at = claimed.leased_until + ChronoDuration::milliseconds(1);
    let report = queue.reap_expired(reap_at).await?;
    assert_eq!(report.expired_domains, vec![locator.thread_key()]);
    assert_eq!(report.recovered_message_ids, vec![claimed.message_id]);

    let snapshot = queue.snapshot_messages().await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].status, "pending");
    assert_eq!(snapshot[0].payload.incoming.content, "recover-me");
    Ok(())
}

#[tokio::test]
async fn queue_reap_recovers_old_active_message_after_domain_is_reacquired() -> Result<()> {
    // 测试场景: 旧 worker 过期后，同 domain 被新 worker 重新获取并 heartbeat，也不能把旧 claim 的 active message 永久续租卡死。
    let queue = TestTopicQueue::new(test_runtime_config());
    let incoming = build_incoming(
        "msg_queue_reacquire",
        "recover-after-reacquire",
        "chat_reap",
    );
    let locator = build_locator(&incoming);
    queue
        .add(
            &locator.thread_key(),
            TopicQueuePayload::new(locator.clone(), incoming.clone()),
        )
        .await?;

    let first_lease = match queue
        .acquire_worker_lease(&locator.thread_key(), "worker-a", Utc::now())
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("first lease should acquire"),
    };
    let claimed = queue
        .claim(&locator.thread_key(), &first_lease, Utc::now())
        .await?
        .expect("message should claim under first lease");

    let reacquired_at = claimed.leased_until + ChronoDuration::milliseconds(1);
    let second_lease = match queue
        .acquire_worker_lease(&locator.thread_key(), "worker-b", reacquired_at)
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("second lease should acquire after expiry"),
    };
    assert!(
        queue
            .heartbeat_worker(
                &second_lease,
                reacquired_at + ChronoDuration::milliseconds(1)
            )
            .await?
    );

    let reap_at = reacquired_at + ChronoDuration::milliseconds(2);
    let report = queue.reap_expired(reap_at).await?;
    assert!(report.expired_domains.is_empty());
    assert_eq!(report.recovered_message_ids, vec![claimed.message_id]);

    let reclaimed = queue
        .claim(&locator.thread_key(), &second_lease, reap_at)
        .await?
        .expect("recovered message should claim again under the new lease");
    assert_eq!(reclaimed.message_id, claimed.message_id);
    assert_eq!(reclaimed.claim_token, second_lease.lease_token);
    Ok(())
}

#[tokio::test]
async fn postgres_queue_roundtrip_and_reap_follow_real_sql_semantics() -> Result<()> {
    // 测试场景: 真实 PostgreSQL queue 必须跑通 add/claim/complete，并能在 domain 被新 lease 重新获取后恢复旧 active message。
    let Some(database_url) = postgres_test_database_url() else {
        eprintln!(
            "skipping postgres queue integration test because OPENJARVIS_TEST_POSTGRES_URL is not set"
        );
        return Ok(());
    };

    reset_postgres_queue_tables(&database_url).await?;
    let queue = PostgresTopicQueue::connect(&database_url, test_runtime_config()).await?;
    queue.initialize_schema().await?;

    let roundtrip_incoming = build_incoming("msg_pg_roundtrip", "roundtrip", "chat_pg_roundtrip");
    let roundtrip_locator = build_locator(&roundtrip_incoming);
    queue
        .add(
            &roundtrip_locator.thread_key(),
            TopicQueuePayload::new(roundtrip_locator.clone(), roundtrip_incoming.clone()),
        )
        .await?;
    let roundtrip_lease = match queue
        .acquire_worker_lease(&roundtrip_locator.thread_key(), "worker-pg-a", Utc::now())
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("roundtrip lease should acquire"),
    };
    let roundtrip_claim = queue
        .claim(
            &roundtrip_locator.thread_key(),
            &roundtrip_lease,
            Utc::now(),
        )
        .await?
        .expect("roundtrip message should claim");
    assert_eq!(roundtrip_claim.payload.incoming.content, "roundtrip");
    assert!(
        queue
            .complete(
                roundtrip_claim.message_id,
                &roundtrip_claim.claim_token,
                Utc::now()
            )
            .await?
    );
    assert!(
        queue
            .claim(
                &roundtrip_locator.thread_key(),
                &roundtrip_lease,
                Utc::now() + ChronoDuration::milliseconds(1)
            )
            .await?
            .is_none()
    );
    assert!(
        queue
            .release_worker(
                &roundtrip_lease,
                Utc::now() + ChronoDuration::milliseconds(2)
            )
            .await?
    );

    let stranded_incoming = build_incoming("msg_pg_reap", "recover-me", "chat_pg_reap");
    let stranded_locator = build_locator(&stranded_incoming);
    queue
        .add(
            &stranded_locator.thread_key(),
            TopicQueuePayload::new(stranded_locator.clone(), stranded_incoming.clone()),
        )
        .await?;
    let first_lease = match queue
        .acquire_worker_lease(&stranded_locator.thread_key(), "worker-pg-b", Utc::now())
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("first stranded lease should acquire"),
    };
    let claimed = queue
        .claim(&stranded_locator.thread_key(), &first_lease, Utc::now())
        .await?
        .expect("stranded message should claim");

    let reacquired_at = claimed.leased_until + ChronoDuration::milliseconds(1);
    let second_lease = match queue
        .acquire_worker_lease(&stranded_locator.thread_key(), "worker-pg-c", reacquired_at)
        .await?
    {
        TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
        TopicQueueLeaseAcquireResult::Busy => panic!("second stranded lease should acquire"),
    };
    assert!(
        queue
            .heartbeat_worker(
                &second_lease,
                reacquired_at + ChronoDuration::milliseconds(1)
            )
            .await?
    );

    let reap_at = reacquired_at + ChronoDuration::milliseconds(2);
    let report = queue.reap_expired(reap_at).await?;
    assert!(
        !report
            .expired_domains
            .contains(&stranded_locator.thread_key())
    );
    assert_eq!(report.recovered_message_ids, vec![claimed.message_id]);

    let reclaimed = queue
        .claim(&stranded_locator.thread_key(), &second_lease, reap_at)
        .await?
        .expect("recovered postgres message should claim again");
    assert_eq!(reclaimed.message_id, claimed.message_id);
    assert_eq!(reclaimed.claim_token, second_lease.lease_token);
    assert!(
        queue
            .complete(reclaimed.message_id, &reclaimed.claim_token, reap_at)
            .await?
    );

    drop(queue);
    reset_postgres_queue_tables(&database_url).await?;
    Ok(())
}
