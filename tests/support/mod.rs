#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    session::{SessionManager, SessionStoreResult, ThreadLocator},
    thread::{Thread, ThreadContextLocator, ThreadToolEvent},
};

pub trait ThreadTestExt {
    fn non_system_messages(&self) -> Vec<ChatMessage>;
    fn system_messages(&self) -> Vec<ChatMessage>;
    fn seed_persisted_messages(&mut self, messages: Vec<ChatMessage>);
    fn commit_test_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    );
    fn commit_test_turn_with_state(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    );
    fn append_open_turn_message(&mut self, message: ChatMessage) -> Result<()>;
    fn replace_non_system_messages_after_compaction(
        &mut self,
        compacted_messages: Vec<ChatMessage>,
    ) -> Result<()>;
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
        let initialized_at = messages
            .iter()
            .find(|message| message.role == ChatMessageRole::System)
            .map(|message| message.created_at);
        self.thread.messages = messages;
        self.thread.request_context_initialized_at = initialized_at;
        self.thread.created_at = created_at;
        self.thread.updated_at = updated_at;
    }

    fn commit_test_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) {
        let reply = messages
            .iter()
            .rev()
            .find(|message| message.role == ChatMessageRole::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        self.begin_turn(external_message_id, started_at)
            .expect("test turn should start");
        for message in messages {
            append_message_without_persist(self, message);
        }
        self.finalize_turn_success(reply, completed_at)
            .expect("test turn should finalize");
    }

    fn commit_test_turn_with_state(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) {
        self.replace_loaded_toolsets(loaded_toolsets);
        let reply = messages
            .iter()
            .rev()
            .find(|message| message.role == ChatMessageRole::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        self.begin_turn(external_message_id, started_at)
            .expect("test turn should start");
        for event in tool_events {
            self.record_tool_event(event)
                .expect("tool event should record inside active test turn");
        }
        for message in messages {
            append_message_without_persist(self, message);
        }
        self.finalize_turn_success(reply, completed_at)
            .expect("test turn should finalize");
    }

    fn append_open_turn_message(&mut self, message: ChatMessage) -> Result<()> {
        append_message_without_persist(self, message);
        Ok(())
    }

    fn replace_non_system_messages_after_compaction(
        &mut self,
        compacted_messages: Vec<ChatMessage>,
    ) -> Result<()> {
        self.replace_messages_after_compaction(compacted_messages)
    }
}

fn append_message_without_persist(thread: &mut Thread, message: ChatMessage) {
    if thread.thread.created_at > message.created_at {
        thread.thread.created_at = message.created_at;
    }
    thread.thread.updated_at = message.created_at;
    thread.thread.messages.push(message);
}

#[derive(Debug, Clone, Default)]
pub struct StoredThreadState {
    pub thread_context: Option<Thread>,
    pub non_system_messages: Vec<ChatMessage>,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
}

#[async_trait]
pub trait SessionManagerTestExt {
    async fn load_non_system_messages(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Vec<ChatMessage>>;
    async fn load_thread_state(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<StoredThreadState>;
    async fn commit_test_turn_messages(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()>;
    async fn commit_test_turn_messages_with_state(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<()>;
    async fn commit_test_turn_messages_with_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: Option<Thread>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()>;
}

#[async_trait]
impl SessionManagerTestExt for SessionManager {
    async fn load_non_system_messages(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Vec<ChatMessage>> {
        Ok(self
            .load_thread_context(locator)
            .await?
            .map(|thread_context| thread_context.non_system_messages())
            .unwrap_or_default())
    }

    async fn load_thread_state(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<StoredThreadState> {
        Ok(self
            .load_thread_context(locator)
            .await?
            .map(|thread_context| StoredThreadState {
                thread_context: Some(thread_context.clone()),
                non_system_messages: thread_context.non_system_messages(),
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.load_tool_events(),
            })
            .unwrap_or_default())
    }

    async fn commit_test_turn_messages(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        self.commit_test_turn_messages_with_state(
            locator,
            external_message_id,
            messages,
            started_at,
            completed_at,
            Vec::new(),
            Vec::new(),
        )
        .await
    }

    async fn commit_test_turn_messages_with_state(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<()> {
        let mut thread_context = self
            .load_thread_context(locator)
            .await?
            .unwrap_or_else(|| Thread::new(ThreadContextLocator::from(locator), completed_at));
        thread_context.commit_test_turn_with_state(
            external_message_id.clone(),
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        );
        self.store_thread_context(locator, thread_context, completed_at)
            .await?;
        if let Some(message_id) = external_message_id.as_deref() {
            self.mark_external_message_processed(locator, message_id, completed_at)
                .await?;
        }
        Ok(())
    }

    async fn commit_test_turn_messages_with_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: Option<Thread>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let mut thread_context = thread_context
            .unwrap_or_else(|| Thread::new(ThreadContextLocator::from(locator), completed_at));
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        thread_context.commit_test_turn(
            external_message_id.clone(),
            messages,
            started_at,
            completed_at,
        );
        self.store_thread_context(locator, thread_context, completed_at)
            .await?;
        if let Some(message_id) = external_message_id.as_deref() {
            self.mark_external_message_processed(locator, message_id, completed_at)
                .await?;
        }
        Ok(())
    }
}
