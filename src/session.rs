//! In-memory session storage keyed by channel, user, and thread.
//!
//! Router owns the session store and uses `load_turn`/`store_turn` to bridge external incoming
//! messages with the agent's normalized chat history. The stored payload is the unified chat
//! protocol message list so providers such as OpenAI and Anthropic can share one persistence
//! layer while keeping provider-specific serialization in the LLM layer.

use crate::context::ChatMessage;
use crate::model::IncomingMessage;
use crate::thread::{
    ConversationThread, ThreadContext, ThreadContextLocator, ThreadToolEvent,
    derive_internal_thread_id,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

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
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create an empty session snapshot.
    pub fn new(key: SessionKey, now: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            key,
            threads: HashMap::new(),
            updated_at: now,
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

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionKey, Session>>,
    strategy: SessionStrategy,
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
            sessions: RwLock::new(HashMap::new()),
            strategy,
        }
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
    pub async fn load_or_create_thread(&self, incoming: &IncomingMessage) -> ThreadLocator {
        let session_key = SessionKey::from_incoming(incoming);
        let external_thread_id = incoming.resolved_external_thread_id();
        let thread_key = session_key.thread_key(&external_thread_id);
        let thread_id = session_key.derive_thread_id(&external_thread_id);
        let now = incoming.received_at;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::new(session_key.clone(), now));
        let session_id = session.id;
        info!(
            session_id = %session_id,
            channel = %incoming.channel,
            user_id = %incoming.user_id,
            external_thread_id = %external_thread_id,
            thread_key = %thread_key,
            thread_id = %thread_id,
            "resolved incoming thread identity"
        );
        let _ = session.load_or_create_thread(
            ThreadContextLocator::new(
                Some(session_id.to_string()),
                incoming.channel.clone(),
                incoming.user_id.clone(),
                external_thread_id.clone(),
                thread_id.to_string(),
            ),
            now,
        );
        session.updated_at = now;

        ThreadLocator::new(session_id, incoming, external_thread_id, thread_id)
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
    pub async fn load_turn(&self, locator: &ThreadLocator) -> Vec<ChatMessage> {
        self.load_thread_state(locator).await.messages
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread_context(&self, locator: &ThreadLocator) -> Option<ThreadContext> {
        let sessions = self.sessions.read().await;
        sessions
            .get(&locator.session_key())
            .and_then(|session| session.threads.get(&locator.thread_id))
            .cloned()
    }

    /// Load the current thread state snapshot, including persisted toolset metadata.
    pub async fn load_thread_state(&self, locator: &ThreadLocator) -> StoredThreadState {
        let sessions = self.sessions.read().await;
        sessions
            .get(&locator.session_key())
            .and_then(|session| session.threads.get(&locator.thread_id))
            .map(|thread_context| StoredThreadState {
                thread_context: Some(thread_context.clone()),
                thread: Some(thread_context.to_conversation_thread()),
                messages: thread_context.load_messages(),
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.load_tool_events(),
            })
            .unwrap_or_default()
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
    ) -> Uuid {
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
    ) -> Uuid {
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
    ) {
        let session_key = locator.session_key();
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::new(session_key, updated_at));
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let _ = session.load_or_create_thread(ThreadContextLocator::from(locator), updated_at);
        session.threads.insert(locator.thread_id, thread_context);
        session.updated_at = updated_at;
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
    ) -> Uuid {
        let session_key = locator.session_key();
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::new(session_key, completed_at));
        let mut thread_context = thread_context.unwrap_or_else(|| {
            session
                .threads
                .get(&locator.thread_id)
                .cloned()
                .unwrap_or_else(|| {
                    ThreadContext::new(ThreadContextLocator::from(locator), completed_at)
                })
        });
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let turn_id =
            thread_context.store_turn(external_message_id, messages, started_at, completed_at);
        self.strategy.retain_thread_messages(&mut thread_context);
        session.threads.insert(locator.thread_id, thread_context);
        session.updated_at = completed_at;
        turn_id
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
    ) -> Uuid {
        let mut thread_context = self.load_thread_context(locator).await.unwrap_or_else(|| {
            ThreadContext::new(ThreadContextLocator::from(locator), completed_at)
        });
        if let Some(active_thread) = active_thread {
            info!(
                thread_id = %locator.thread_id,
                turn_count = active_thread.turns.len(),
                "replacing active thread history before storing turn"
            );
            thread_context.overwrite_active_history_from_conversation_thread(&active_thread);
        }
        let turn_id = thread_context.store_turn_state(
            external_message_id,
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        );
        self.strategy.retain_thread_messages(&mut thread_context);
        self.store_thread_context(locator, thread_context, completed_at)
            .await;
        turn_id
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(key).cloned()
    }
}
