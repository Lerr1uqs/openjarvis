#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    queue::{
        ClaimedTopicQueueMessage, TopicQueue, TopicQueueLeaseAcquireResult, TopicQueueMessage,
        TopicQueuePayload, TopicQueueReapReport, TopicQueueRuntimeConfig, TopicQueueWorkerLease,
    },
    thread::Thread,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

pub trait ThreadTestExt {
    fn non_system_messages(&self) -> Vec<ChatMessage>;
    fn system_messages(&self) -> Vec<ChatMessage>;
    fn seed_persisted_messages(&mut self, messages: Vec<ChatMessage>);
    fn append_persisted_messages_for_test(&mut self, messages: Vec<ChatMessage>);
    fn append_persisted_messages_with_state_for_test(
        &mut self,
        messages: Vec<ChatMessage>,
        loaded_toolsets: Vec<String>,
    );
    fn append_unpersisted_message_for_test(&mut self, message: ChatMessage) -> Result<()>;
}

impl ThreadTestExt for Thread {
    fn non_system_messages(&self) -> Vec<ChatMessage> {
        self.messages()
            .into_iter()
            .filter(|message| message.role != ChatMessageRole::System)
            .collect()
    }

    fn system_messages(&self) -> Vec<ChatMessage> {
        self.messages()
            .into_iter()
            .filter(|message| message.role == ChatMessageRole::System)
            .collect()
    }

    fn seed_persisted_messages(&mut self, messages: Vec<ChatMessage>) {
        let created_at = messages
            .first()
            .map(|message| message.created_at)
            .unwrap_or_else(Utc::now);
        let updated_at = messages
            .last()
            .map(|message| message.created_at)
            .unwrap_or(created_at);
        self.thread.messages = messages;
        self.thread.created_at = created_at;
        self.thread.updated_at = updated_at;
        self.state.lifecycle.initialized = self
            .thread
            .messages
            .iter()
            .any(|message| message.role == ChatMessageRole::System);
    }

    fn append_persisted_messages_for_test(&mut self, messages: Vec<ChatMessage>) {
        for message in messages {
            append_message_without_persist(self, message);
        }
    }

    fn append_persisted_messages_with_state_for_test(
        &mut self,
        messages: Vec<ChatMessage>,
        loaded_toolsets: Vec<String>,
    ) {
        self.replace_loaded_toolsets(loaded_toolsets);
        for message in messages {
            append_message_without_persist(self, message);
        }
    }

    fn append_unpersisted_message_for_test(&mut self, message: ChatMessage) -> Result<()> {
        append_message_without_persist(self, message);
        Ok(())
    }
}

fn append_message_without_persist(thread: &mut Thread, message: ChatMessage) {
    if thread.thread.created_at > message.created_at {
        thread.thread.created_at = message.created_at;
    }
    thread.thread.updated_at = message.created_at;
    thread.thread.messages.push(message);
}

#[derive(Debug, Clone)]
pub struct TestTopicQueueMessageSnapshot {
    pub message_id: Uuid,
    pub topic: String,
    pub status: String,
    pub payload: TopicQueuePayload,
}

#[derive(Debug, Clone)]
struct TestTopicQueueMessageState {
    message_id: Uuid,
    topic: String,
    payload: TopicQueuePayload,
    status: String,
    claim_token: Option<String>,
    leased_until: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    claimed_at: Option<chrono::DateTime<Utc>>,
    completed_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct TestTopicQueueWorkerState {
    worker_id: String,
    domain: String,
    lease_token: String,
    lease_expires_at: chrono::DateTime<Utc>,
    last_heartbeat_at: chrono::DateTime<Utc>,
    started_at: chrono::DateTime<Utc>,
    stopped_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct TestTopicQueueState {
    messages: Vec<TestTopicQueueMessageState>,
    workers: HashMap<String, TestTopicQueueWorkerState>,
}

#[derive(Debug, Clone)]
pub struct TestTopicQueue {
    runtime_config: TopicQueueRuntimeConfig,
    state: Arc<Mutex<TestTopicQueueState>>,
}

impl Default for TestTopicQueue {
    fn default() -> Self {
        Self::new(TopicQueueRuntimeConfig::default())
    }
}

impl TestTopicQueue {
    pub fn new(runtime_config: TopicQueueRuntimeConfig) -> Self {
        Self {
            runtime_config,
            state: Arc::new(Mutex::new(TestTopicQueueState::default())),
        }
    }

    pub async fn snapshot_messages(&self) -> Vec<TestTopicQueueMessageSnapshot> {
        self.state
            .lock()
            .await
            .messages
            .iter()
            .map(|message| TestTopicQueueMessageSnapshot {
                message_id: message.message_id,
                topic: message.topic.clone(),
                status: message.status.clone(),
                payload: message.payload.clone(),
            })
            .collect()
    }

    pub async fn active_worker_domains(&self, now: chrono::DateTime<Utc>) -> Vec<String> {
        self.state
            .lock()
            .await
            .workers
            .values()
            .filter(|worker| worker.stopped_at.is_none() && worker.lease_expires_at > now)
            .map(|worker| worker.domain.clone())
            .collect()
    }
}

#[async_trait]
impl TopicQueue for TestTopicQueue {
    fn runtime_config(&self) -> TopicQueueRuntimeConfig {
        self.runtime_config
    }

    async fn initialize_schema(&self) -> Result<()> {
        Ok(())
    }

    async fn add(&self, topic: &str, payload: TopicQueuePayload) -> Result<TopicQueueMessage> {
        let created_at = Utc::now();
        let message = TestTopicQueueMessageState {
            message_id: Uuid::new_v4(),
            topic: topic.to_string(),
            payload: payload.clone(),
            status: "pending".to_string(),
            claim_token: None,
            leased_until: None,
            created_at,
            claimed_at: None,
            completed_at: None,
        };
        self.state.lock().await.messages.push(message.clone());
        Ok(TopicQueueMessage {
            message_id: message.message_id,
            topic: message.topic,
            payload,
            created_at,
        })
    }

    async fn claim(
        &self,
        domain: &str,
        lease: &TopicQueueWorkerLease,
        now: chrono::DateTime<Utc>,
    ) -> Result<Option<ClaimedTopicQueueMessage>> {
        let mut state = self.state.lock().await;
        let Some(worker) = state.workers.get(domain) else {
            return Ok(None);
        };
        if worker.worker_id != lease.worker_id
            || worker.lease_token != lease.lease_token
            || worker.stopped_at.is_some()
            || worker.lease_expires_at <= now
        {
            return Ok(None);
        }

        let leased_until = now
            + chrono::Duration::from_std(self.runtime_config.lease_ttl)
                .expect("test queue lease ttl should fit chrono");
        let Some(message) = state
            .messages
            .iter_mut()
            .filter(|message| message.topic == domain && message.status == "pending")
            .min_by_key(|message| (message.created_at, message.message_id))
        else {
            return Ok(None);
        };
        message.status = "active".to_string();
        message.claim_token = Some(lease.lease_token.clone());
        message.leased_until = Some(leased_until);
        message.claimed_at = Some(now);
        Ok(Some(ClaimedTopicQueueMessage {
            message_id: message.message_id,
            topic: message.topic.clone(),
            payload: message.payload.clone(),
            claim_token: lease.lease_token.clone(),
            leased_until,
            created_at: message.created_at,
            claimed_at: now,
        }))
    }

    async fn complete(
        &self,
        message_id: Uuid,
        claim_token: &str,
        completed_at: chrono::DateTime<Utc>,
    ) -> Result<bool> {
        let mut state = self.state.lock().await;
        let Some(message) = state
            .messages
            .iter_mut()
            .find(|message| message.message_id == message_id)
        else {
            return Ok(false);
        };
        if message.status != "active" || message.claim_token.as_deref() != Some(claim_token) {
            return Ok(false);
        }
        message.status = "complete".to_string();
        message.claim_token = None;
        message.leased_until = None;
        message.completed_at = Some(completed_at);
        Ok(true)
    }

    async fn acquire_worker_lease(
        &self,
        domain: &str,
        worker_id: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<TopicQueueLeaseAcquireResult> {
        let mut state = self.state.lock().await;
        if let Some(worker) = state.workers.get(domain)
            && worker.stopped_at.is_none()
            && worker.lease_expires_at > now
        {
            return Ok(TopicQueueLeaseAcquireResult::Busy);
        }

        let lease_expires_at = now
            + chrono::Duration::from_std(self.runtime_config.lease_ttl)
                .expect("test queue lease ttl should fit chrono");
        let lease = TopicQueueWorkerLease {
            worker_id: worker_id.to_string(),
            domain: domain.to_string(),
            lease_token: Uuid::new_v4().to_string(),
            lease_expires_at,
            started_at: now,
        };
        state.workers.insert(
            domain.to_string(),
            TestTopicQueueWorkerState {
                worker_id: lease.worker_id.clone(),
                domain: lease.domain.clone(),
                lease_token: lease.lease_token.clone(),
                lease_expires_at,
                last_heartbeat_at: now,
                started_at: now,
                stopped_at: None,
            },
        );
        Ok(TopicQueueLeaseAcquireResult::Acquired(lease))
    }

    async fn heartbeat_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        now: chrono::DateTime<Utc>,
    ) -> Result<bool> {
        let mut state = self.state.lock().await;
        let Some(worker) = state.workers.get_mut(&lease.domain) else {
            return Ok(false);
        };
        if worker.worker_id != lease.worker_id
            || worker.lease_token != lease.lease_token
            || worker.stopped_at.is_some()
        {
            return Ok(false);
        }

        let lease_expires_at = now
            + chrono::Duration::from_std(self.runtime_config.lease_ttl)
                .expect("test queue lease ttl should fit chrono");
        worker.lease_expires_at = lease_expires_at;
        worker.last_heartbeat_at = now;
        for message in state.messages.iter_mut().filter(|message| {
            message.topic == lease.domain
                && message.status == "active"
                && message.claim_token.as_deref() == Some(lease.lease_token.as_str())
        }) {
            message.leased_until = Some(lease_expires_at);
        }
        Ok(true)
    }

    async fn release_worker(
        &self,
        lease: &TopicQueueWorkerLease,
        released_at: chrono::DateTime<Utc>,
    ) -> Result<bool> {
        let mut state = self.state.lock().await;
        let Some(worker) = state.workers.get_mut(&lease.domain) else {
            return Ok(false);
        };
        if worker.worker_id != lease.worker_id || worker.lease_token != lease.lease_token {
            return Ok(false);
        }
        worker.stopped_at = Some(released_at);
        worker.lease_expires_at = released_at;
        worker.last_heartbeat_at = released_at;
        Ok(true)
    }

    async fn reap_expired(&self, now: chrono::DateTime<Utc>) -> Result<TopicQueueReapReport> {
        let mut state = self.state.lock().await;
        let expired_domains = state
            .workers
            .values_mut()
            .filter(|worker| worker.stopped_at.is_none() && worker.lease_expires_at <= now)
            .map(|worker| {
                worker.stopped_at = Some(now);
                worker.lease_expires_at = now;
                worker.domain.clone()
            })
            .collect::<Vec<_>>();
        let mut recovered_message_ids = Vec::new();
        for message in state.messages.iter_mut() {
            if message.status == "active"
                && message
                    .leased_until
                    .is_some_and(|leased_until| leased_until <= now)
            {
                message.status = "pending".to_string();
                message.claim_token = None;
                message.leased_until = None;
                message.claimed_at = None;
                recovered_message_ids.push(message.message_id);
            }
        }
        Ok(TopicQueueReapReport {
            expired_domains,
            recovered_message_ids,
        })
    }

    async fn pending_topics(&self, limit: usize) -> Result<Vec<String>> {
        let state = self.state.lock().await;
        let mut topics = state
            .messages
            .iter()
            .filter(|message| message.status == "pending")
            .fold(
                HashMap::<String, chrono::DateTime<Utc>>::new(),
                |mut acc, message| {
                    acc.entry(message.topic.clone())
                        .and_modify(|current| {
                            if message.created_at < *current {
                                *current = message.created_at;
                            }
                        })
                        .or_insert(message.created_at);
                    acc
                },
            )
            .into_iter()
            .collect::<Vec<_>>();
        topics.sort_by_key(|(topic, created_at)| (*created_at, topic.clone()));
        Ok(topics
            .into_iter()
            .take(limit)
            .map(|(topic, _)| topic)
            .collect())
    }

    async fn is_worker_active(&self, domain: &str, now: chrono::DateTime<Utc>) -> Result<bool> {
        let state = self.state.lock().await;
        Ok(state
            .workers
            .get(domain)
            .is_some_and(|worker| worker.stopped_at.is_none() && worker.lease_expires_at > now))
    }
}
