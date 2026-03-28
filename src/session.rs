//! Session cache and persistent thread-context storage orchestration.
//!
//! `SessionManager` owns the hot in-process cache for active sessions and delegates durable
//! persistence to a pluggable `SessionStore` backend. The runtime only reads and writes
//! `ThreadContext` snapshots through this boundary so thread recovery, tool state restoration,
//! and external-message deduplication stay consistent across restarts.

pub mod store;

use crate::context::ChatMessage;
use crate::model::IncomingMessage;
use crate::thread::{
    ConversationThread, ThreadContext, ThreadContextLocator, ThreadToolEvent,
    derive_internal_thread_id,
};
use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::info;
use uuid::Uuid;

pub use store::{
    ExternalMessageDedupRecord, MemorySessionStore, SessionRevisionConflict, SessionStore,
    SessionStoreError, SessionStoreResult, SqliteSessionStore, StoredSessionRecord,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionKey {
    pub channel: String,
    pub user_id: String,
}

impl SessionKey {
    /// Build a stable session key from a normalized incoming message.
    pub fn from_incoming(incoming: &IncomingMessage) -> Self {
        Self {
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
        }
    }

    /// Return the normalized thread key for one external thread inside this session.
    ///
    /// `thread_key` follows the contract `user:channel:external_thread_id`.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionKey;
    ///
    /// let key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    ///
    /// assert_eq!(key.thread_key("thread_ext"), "ou_xxx:feishu:thread_ext");
    /// ```
    pub fn thread_key(&self, external_thread_id: &str) -> String {
        format!("{}:{}:{}", self.user_id, self.channel, external_thread_id)
    }

    /// Derive the stable internal thread id for one external thread inside this session.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionKey;
    ///
    /// let key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    ///
    /// let thread_id = key.derive_thread_id("thread_ext");
    /// assert_eq!(thread_id, key.derive_thread_id("thread_ext"));
    /// ```
    pub fn derive_thread_id(&self, external_thread_id: &str) -> Uuid {
        derive_internal_thread_id(&self.thread_key(external_thread_id))
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ThreadLocator {
    pub session_id: Uuid,
    pub channel: String,
    pub user_id: String,
    pub external_thread_id: String,
    pub thread_id: Uuid,
}

impl ThreadLocator {
    /// Build a resolved session-thread locator from one incoming message and the derived ids.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::{SessionKey, ThreadLocator};
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: None,
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let session_id = Uuid::new_v4();
    /// let thread_id = SessionKey::from_incoming(&incoming).derive_thread_id("default");
    /// let locator = ThreadLocator::new(session_id, &incoming, "default", thread_id);
    /// assert_eq!(locator.external_thread_id, "default");
    /// assert_eq!(locator.thread_id, thread_id);
    /// assert_eq!(locator.thread_key(), "ou_xxx:feishu:default");
    /// ```
    pub fn new(
        session_id: Uuid,
        incoming: &IncomingMessage,
        external_thread_id: impl Into<String>,
        thread_id: Uuid,
    ) -> Self {
        Self {
            session_id,
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
            external_thread_id: external_thread_id.into(),
            thread_id,
        }
    }

    /// Return the parent session key for this thread locator.
    pub fn session_key(&self) -> SessionKey {
        SessionKey {
            channel: self.channel.clone(),
            user_id: self.user_id.clone(),
        }
    }

    /// Return the normalized thread key for this resolved thread locator.
    pub fn thread_key(&self) -> String {
        self.session_key().thread_key(&self.external_thread_id)
    }
}

impl From<&ThreadLocator> for ThreadContextLocator {
    fn from(value: &ThreadLocator) -> Self {
        Self::new(
            Some(value.session_id.to_string()),
            value.channel.clone(),
            value.user_id.clone(),
            value.external_thread_id.clone(),
            value.thread_id.to_string(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub key: SessionKey,
    pub threads: HashMap<Uuid, ThreadContext>,
    created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create an empty session snapshot.
    pub fn new(key: SessionKey, now: DateTime<Utc>) -> Self {
        Self::with_id(Uuid::new_v4(), key, now)
    }

    fn with_id(id: Uuid, key: SessionKey, now: DateTime<Utc>) -> Self {
        Self {
            id,
            key,
            threads: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    fn from_record(record: StoredSessionRecord) -> Self {
        Self {
            id: record.id,
            key: record.key,
            threads: HashMap::new(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }

    /// Resolve one normalized external thread id into an internal thread context.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{session::{Session, SessionKey}, thread::ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let session_key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    /// let mut session = Session::new(session_key.clone(), now);
    /// let thread_id = session_key.derive_thread_id("default");
    /// let thread = session.load_or_create_thread(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
    ///     now,
    /// );
    ///
    /// assert_eq!(thread.locator.thread_id, thread_id.to_string());
    /// assert_eq!(thread.locator.external_thread_id, "default");
    /// assert_eq!(thread.locator.thread_key(), "ou_xxx:feishu:default");
    /// ```
    pub fn load_or_create_thread(
        &mut self,
        locator: ThreadContextLocator,
        now: DateTime<Utc>,
    ) -> &mut ThreadContext {
        let thread_id = Uuid::parse_str(&locator.thread_id)
            .expect("thread context locator should carry a UUID thread_id");
        self.threads
            .entry(thread_id)
            .or_insert_with(|| ThreadContext::new(locator, now))
    }

    /// Return the internal thread id currently bound to one normalized external thread id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{session::{Session, SessionKey}, thread::ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let session_key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    /// let mut session = Session::new(session_key.clone(), now);
    /// let thread_id = session_key.derive_thread_id("default");
    /// session.load_or_create_thread(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
    ///     now,
    /// );
    ///
    /// assert_eq!(session.thread_id_for_external("default"), Some(thread_id));
    /// ```
    pub fn thread_id_for_external(&self, external_thread_id: &str) -> Option<Uuid> {
        let thread_id = self.key.derive_thread_id(external_thread_id);
        self.threads.contains_key(&thread_id).then_some(thread_id)
    }
}

#[derive(Debug, Clone)]
pub struct SessionStrategy {
    pub max_messages_per_thread: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StoredThreadState {
    pub thread_context: Option<ThreadContext>,
    pub thread: Option<ConversationThread>,
    pub messages: Vec<ChatMessage>,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
}

impl Default for SessionStrategy {
    fn default() -> Self {
        Self {
            max_messages_per_thread: 10,
        }
    }
}

impl SessionStrategy {
    /// Apply the current thread-storage policy to one thread context conversation.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::session::SessionStrategy;
    /// use openjarvis::thread::{ThreadContext, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let thread_id = openjarvis::thread::derive_internal_thread_id("ou_xxx:feishu:default");
    /// let mut thread = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
    ///     now,
    /// );
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_0", now),
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_1", now),
    ///     ],
    ///     now,
    ///     now,
    /// );
    ///
    /// SessionStrategy {
    ///     max_messages_per_thread: 1,
    /// }
    /// .retain_thread_messages(&mut thread);
    ///
    /// assert_eq!(thread.load_messages().len(), 1);
    /// ```
    pub fn retain_thread_messages(&self, thread: &mut ThreadContext) {
        thread.retain_latest_messages(self.max_messages_per_thread);
    }
}

pub struct SessionManager {
    sessions: Mutex<HashMap<SessionKey, Session>>,
    strategy: SessionStrategy,
    store: Arc<dyn SessionStore>,
}

impl SessionManager {
    /// Create an empty in-memory session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionManager;
    ///
    /// let manager = SessionManager::new();
    /// assert_eq!(manager.strategy().max_messages_per_thread, 10);
    /// ```
    pub fn new() -> Self {
        Self::with_strategy(SessionStrategy::default())
    }

    /// Create an empty in-memory session manager with a custom storage strategy.
    pub fn with_strategy(strategy: SessionStrategy) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            strategy,
            store: Arc::new(MemorySessionStore::new()),
        }
    }

    /// Create a session manager backed by the provided persistent store.
    pub async fn with_store(
        store: Arc<dyn SessionStore>,
        strategy: SessionStrategy,
    ) -> SessionStoreResult<Self> {
        store.initialize_schema().await?;
        Ok(Self {
            sessions: Mutex::new(HashMap::new()),
            strategy,
            store,
        })
    }

    /// Return the configured session storage strategy.
    pub fn strategy(&self) -> &SessionStrategy {
        &self.strategy
    }

    /// Resolve the external thread id on one incoming message and create the session/thread on
    /// first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::SessionManager;
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_1".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let _future = manager.load_or_create_thread(&incoming);
    /// ```
    pub async fn load_or_create_thread(
        &self,
        incoming: &IncomingMessage,
    ) -> SessionStoreResult<ThreadLocator> {
        let session_key = SessionKey::from_incoming(incoming);
        let external_thread_id = incoming.resolved_external_thread_id();
        let thread_key = session_key.thread_key(&external_thread_id);
        let thread_id = session_key.derive_thread_id(&external_thread_id);
        let now = incoming.received_at;
        let session_record = self
            .store
            .resolve_or_create_session(&session_key, now)
            .await?;
        let locator = ThreadLocator::new(session_record.id, incoming, external_thread_id, thread_id);

        info!(
            session_id = %locator.session_id,
            channel = %incoming.channel,
            user_id = %incoming.user_id,
            external_thread_id = %locator.external_thread_id,
            thread_key = %thread_key,
            thread_id = %locator.thread_id,
            "resolved incoming thread identity"
        );

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::from_record(session_record.clone()));
        if !session.threads.contains_key(&locator.thread_id) {
            if let Some(mut stored_thread) = self.store.load_thread_context(&locator).await? {
                stored_thread.rebind_locator(ThreadContextLocator::from(&locator));
                session.threads.insert(locator.thread_id, stored_thread);
                info!(
                    session_id = %locator.session_id,
                    thread_id = %locator.thread_id,
                    "restored thread context from session store"
                );
            } else {
                let _ = session.load_or_create_thread(ThreadContextLocator::from(&locator), now);
                info!(
                    session_id = %locator.session_id,
                    thread_id = %locator.thread_id,
                    "created new thread context in session cache"
                );
            }
        }
        session.updated_at = now;

        Ok(locator)
    }

    /// Load the current flattened history for one channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     session_id: Uuid::new_v4(),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     external_thread_id: "default".to_string(),
    ///     thread_id: Uuid::new_v4(),
    /// };
    ///
    /// let _future = manager.load_turn(&locator);
    /// ```
    pub async fn load_turn(&self, locator: &ThreadLocator) -> SessionStoreResult<Vec<ChatMessage>> {
        Ok(self.load_thread_state(locator).await?.messages)
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<ThreadContext>> {
        let session_key = locator.session_key();
        let mut sessions = self.sessions.lock().await;
        if let Some(thread_context) = sessions
            .get(&session_key)
            .and_then(|session| session.threads.get(&locator.thread_id))
            .cloned()
        {
            return Ok(Some(thread_context));
        }

        if !sessions.contains_key(&session_key) {
            let Some(session_record) = self.store.load_session(&session_key).await? else {
                return Ok(None);
            };
            sessions.insert(session_key.clone(), Session::from_record(session_record));
        }

        let session = sessions
            .get_mut(&session_key)
            .expect("session should exist after store load");
        if let Some(thread_context) = session.threads.get(&locator.thread_id).cloned() {
            return Ok(Some(thread_context));
        }

        let Some(mut stored_thread) = self.store.load_thread_context(locator).await? else {
            return Ok(None);
        };
        stored_thread.rebind_locator(ThreadContextLocator::from(locator));
        session
            .threads
            .insert(locator.thread_id, stored_thread.clone());
        Ok(Some(stored_thread))
    }

    /// Load the current thread state snapshot, including persisted toolset metadata.
    pub async fn load_thread_state(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<StoredThreadState> {
        Ok(self
            .load_thread_context(locator)
            .await?
            .map(|thread_context| StoredThreadState {
                thread_context: Some(thread_context.clone()),
                thread: Some(thread_context.to_conversation_thread()),
                messages: thread_context.load_messages(),
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.load_tool_events(),
            })
            .unwrap_or_default())
    }

    /// Return whether one external message was already fully processed for the target thread.
    pub async fn is_external_message_processed(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<bool> {
        Ok(self
            .store
            .load_external_message_record(locator, external_message_id)
            .await?
            .is_some())
    }

    /// Persist one completed external-message deduplication record without appending a new turn.
    pub async fn mark_external_message_processed(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
        turn_id: Option<Uuid>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let record = ExternalMessageDedupRecord {
            thread_id: locator.thread_id,
            external_message_id: external_message_id.to_string(),
            turn_id,
            completed_at,
        };
        self.store.save_external_message_record(locator, &record).await
    }

    /// Store one completed turn for the provided channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     session_id: Uuid::new_v4(),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     external_thread_id: "default".to_string(),
    ///     thread_id: Uuid::new_v4(),
    /// };
    /// let _future = manager.store_turn(
    ///     &locator,
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())],
    ///     Utc::now(),
    ///     Utc::now(),
    /// );
    /// ```
    pub async fn store_turn(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<Uuid> {
        self.store_turn_with_state(
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

    /// Store one completed turn together with persisted thread tool runtime metadata.
    pub async fn store_turn_with_state(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<Uuid> {
        self.store_turn_with_active_thread(
            locator,
            None,
            external_message_id,
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        )
        .await
    }

    /// Persist one updated thread context without appending a new turn.
    pub async fn store_thread_context(
        &self,
        locator: &ThreadLocator,
        mut thread_context: ThreadContext,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let session_key = locator.session_key();
        let session_record = self
            .store
            .resolve_or_create_session(&session_key, updated_at)
            .await?;
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        match self
            .store
            .save_thread_context(&thread_context, updated_at, None)
            .await
        {
            Ok(new_revision) => {
                thread_context.set_revision(new_revision);
            }
            Err(SessionStoreError::RevisionConflict(_)) => {
                let Some(latest) = self.reload_thread_context_from_store(locator).await? else {
                    return Err(SessionStoreError::Other(anyhow::anyhow!(
                        "thread `{}` disappeared during conflict recovery",
                        locator.thread_id
                    )));
                };
                let mut merged = merge_state_only_conflict_resolution(latest, &thread_context);
                let new_revision = self
                    .store
                    .save_thread_context(&merged, updated_at, None)
                    .await?;
                merged.set_revision(new_revision);
                thread_context = merged;
            }
            Err(error) => return Err(error),
        }

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::from_record(session_record));
        session.threads.insert(locator.thread_id, thread_context);
        session.updated_at = updated_at;
        Ok(())
    }

    /// Store one completed turn after optionally replacing the current active thread context.
    pub async fn store_turn_with_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: Option<ThreadContext>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<Uuid> {
        let session_key = locator.session_key();
        let session_record = self
            .store
            .resolve_or_create_session(&session_key, completed_at)
            .await?;
        let mut thread_context = match thread_context {
            Some(thread_context) => thread_context,
            None => self
                .load_thread_context(locator)
                .await?
                .unwrap_or_else(|| ThreadContext::new(ThreadContextLocator::from(locator), completed_at)),
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let turn_id =
            thread_context.store_turn(external_message_id.clone(), messages, started_at, completed_at);
        self.strategy.retain_thread_messages(&mut thread_context);
        let dedup_record = external_message_id.as_ref().map(|message_id| ExternalMessageDedupRecord {
            thread_id: locator.thread_id,
            external_message_id: message_id.clone(),
            turn_id: Some(turn_id),
            completed_at,
        });
        thread_context = self
            .save_turn_snapshot_with_retry(locator, thread_context, completed_at, dedup_record.as_ref())
            .await?;

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::from_record(session_record));
        session.threads.insert(locator.thread_id, thread_context);
        session.updated_at = completed_at;
        Ok(turn_id)
    }

    /// Store one completed turn after optionally replacing the current active thread history.
    pub async fn store_turn_with_active_thread(
        &self,
        locator: &ThreadLocator,
        active_thread: Option<ConversationThread>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<Uuid> {
        let session_key = locator.session_key();
        let session_record = self
            .store
            .resolve_or_create_session(&session_key, completed_at)
            .await?;
        let mut thread_context = self
            .load_thread_context(locator)
            .await?
            .unwrap_or_else(|| ThreadContext::new(ThreadContextLocator::from(locator), completed_at));
        if let Some(active_thread) = active_thread {
            info!(
                thread_id = %locator.thread_id,
                turn_count = active_thread.turns.len(),
                "replacing active thread history before storing turn"
            );
            thread_context.overwrite_active_history_from_conversation_thread(&active_thread);
        }
        let turn_id = thread_context.store_turn_state(
            external_message_id.clone(),
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        );
        self.strategy.retain_thread_messages(&mut thread_context);
        let dedup_record = external_message_id.as_ref().map(|message_id| ExternalMessageDedupRecord {
            thread_id: locator.thread_id,
            external_message_id: message_id.clone(),
            turn_id: Some(turn_id),
            completed_at,
        });
        thread_context = self
            .save_turn_snapshot_with_retry(locator, thread_context, completed_at, dedup_record.as_ref())
            .await?;

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::from_record(session_record));
        session.threads.insert(locator.thread_id, thread_context);
        session.updated_at = completed_at;
        Ok(turn_id)
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let sessions = self.sessions.lock().await;
        sessions.get(key).map(|session| {
            let mut session = session.clone();
            session.updated_at = session.updated_at.max(session.created_at);
            session
        })
    }

    async fn reload_thread_context_from_store(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<ThreadContext>> {
        let session_key = locator.session_key();
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(&session_key) {
            if let Some(session_record) = self.store.load_session(&session_key).await? {
                sessions.insert(session_key.clone(), Session::from_record(session_record));
            } else {
                return Ok(None);
            }
        }

        let Some(mut thread_context) = self.store.load_thread_context(locator).await? else {
            return Ok(None);
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let session = sessions
            .get_mut(&session_key)
            .expect("session should exist before store reload");
        session
            .threads
            .insert(locator.thread_id, thread_context.clone());
        Ok(Some(thread_context))
    }

    async fn save_turn_snapshot_with_retry(
        &self,
        locator: &ThreadLocator,
        thread_context: ThreadContext,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<ThreadContext> {
        match self
            .store
            .save_thread_context(&thread_context, updated_at, dedup_record)
            .await
        {
            Ok(new_revision) => {
                let mut thread_context = thread_context;
                thread_context.set_revision(new_revision);
                Ok(thread_context)
            }
            Err(SessionStoreError::RevisionConflict(_)) => {
                let Some(latest) = self.reload_thread_context_from_store(locator).await? else {
                    return Err(SessionStoreError::Other(anyhow::anyhow!(
                        "thread `{}` disappeared during turn conflict recovery",
                        locator.thread_id
                    )));
                };
                let mut merged = merge_turn_conflict_resolution(latest, thread_context);
                let new_revision = self
                    .store
                    .save_thread_context(&merged, updated_at, dedup_record)
                    .await?;
                merged.set_revision(new_revision);
                Ok(merged)
            }
            Err(error) => Err(error),
        }
    }
}

fn merge_state_only_conflict_resolution(
    mut latest: ThreadContext,
    desired: &ThreadContext,
) -> ThreadContext {
    if desired.state.features.compact_enabled_override.is_some()
        || desired.state.features.auto_compact_override.is_some()
    {
        latest.state.features = desired.state.features.clone();
    }
    if !desired.state.approval.pending.is_empty() || !desired.state.approval.decisions.is_empty() {
        latest.state.approval = desired.state.approval.clone();
    }
    latest.rebind_locator(desired.locator.clone());
    latest
}

fn merge_turn_conflict_resolution(
    latest: ThreadContext,
    mut pending_turn_snapshot: ThreadContext,
) -> ThreadContext {
    pending_turn_snapshot.state.features = latest.state.features.clone();
    pending_turn_snapshot.state.approval = latest.state.approval.clone();
    pending_turn_snapshot.set_revision(latest.revision());
    pending_turn_snapshot
}
