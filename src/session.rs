//! In-memory session storage keyed by channel, user, and thread.
//!
//! Router owns the session store and uses `load_turn`/`store_turn` to bridge external incoming
//! messages with the agent's normalized chat history. The stored payload is the unified chat
//! protocol message list so providers such as OpenAI and Anthropic can share one persistence
//! layer while keeping provider-specific serialization in the LLM layer.

use crate::context::ChatMessage;
use crate::model::IncomingMessage;
use crate::thread::{ConversationThread, ThreadToolEvent};
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
    /// Build a resolved session-thread locator from one incoming message and the stored ids.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::ThreadLocator;
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
    ///     thread_id: None,
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
    /// let thread_id = Uuid::new_v4();
    /// let locator = ThreadLocator::new(session_id, &incoming, "default", thread_id);
    /// assert_eq!(locator.external_thread_id, "default");
    /// assert_eq!(locator.thread_id, thread_id);
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
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub external_thread_index: HashMap<String, Uuid>,
    pub threads: HashMap<Uuid, ConversationThread>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create an empty session snapshot.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            external_thread_index: HashMap::new(),
            threads: HashMap::new(),
            updated_at: now,
        }
    }

    /// Resolve one normalized external thread id into an internal conversation thread.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::session::Session;
    /// use uuid::Uuid;
    ///
    /// let now = Utc::now();
    /// let mut session = Session::new(now);
    /// let preferred_thread_id = Uuid::new_v4();
    /// let thread = session.load_or_create_thread("default", preferred_thread_id, now);
    ///
    /// assert_eq!(thread.id, preferred_thread_id);
    /// assert_eq!(thread.external_thread_id, "default");
    /// ```
    pub fn load_or_create_thread(
        &mut self,
        external_thread_id: impl Into<String>,
        preferred_thread_id: Uuid,
        now: DateTime<Utc>,
    ) -> &mut ConversationThread {
        let external_thread_id = external_thread_id.into();
        let thread_id = *self
            .external_thread_index
            .entry(external_thread_id.clone())
            .or_insert(preferred_thread_id);

        self.threads
            .entry(thread_id)
            .or_insert_with(|| ConversationThread::with_id(thread_id, external_thread_id, now))
    }

    /// Return the internal thread id currently bound to one normalized external thread id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::session::Session;
    /// use uuid::Uuid;
    ///
    /// let now = Utc::now();
    /// let mut session = Session::new(now);
    /// let thread_id = Uuid::new_v4();
    /// session.load_or_create_thread("default", thread_id, now);
    ///
    /// assert_eq!(session.thread_id_for_external("default"), Some(thread_id));
    /// ```
    pub fn thread_id_for_external(&self, external_thread_id: &str) -> Option<Uuid> {
        self.external_thread_index.get(external_thread_id).copied()
    }
}

#[derive(Debug, Clone)]
pub struct SessionStrategy {
    pub max_messages_per_thread: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StoredThreadState {
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
    /// Apply the current thread-storage policy to one conversation thread.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::session::SessionStrategy;
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
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
    pub fn retain_thread_messages(&self, thread: &mut ConversationThread) {
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
    ///     thread_id: None,
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
        let now = incoming.received_at;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::new(now));
        let session_id = session.id;
        let preferred_thread_id = session
            .thread_id_for_external(&external_thread_id)
            .unwrap_or_else(Uuid::new_v4);
        let thread_id = session
            .load_or_create_thread(external_thread_id.clone(), preferred_thread_id, now)
            .id;
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

    /// Load the current thread state snapshot, including persisted toolset metadata.
    pub async fn load_thread_state(&self, locator: &ThreadLocator) -> StoredThreadState {
        let sessions = self.sessions.read().await;
        sessions
            .get(&locator.session_key())
            .and_then(|session| session.threads.get(&locator.thread_id))
            .map(|thread| StoredThreadState {
                thread: Some(thread.clone()),
                messages: thread.load_messages(),
                loaded_toolsets: thread.load_toolsets(),
                tool_events: thread.load_tool_events(),
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
        let session_key = locator.session_key();
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::new(completed_at));
        let thread = session.load_or_create_thread(
            locator.external_thread_id.clone(),
            locator.thread_id,
            completed_at,
        );
        if let Some(active_thread) = active_thread {
            info!(
                thread_id = %locator.thread_id,
                turn_count = active_thread.turns.len(),
                "replacing active thread history before storing turn"
            );
            thread.overwrite_active_history(&active_thread);
        }
        let turn_id = thread.store_turn_state(
            external_message_id,
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        );
        self.strategy.retain_thread_messages(thread);
        session.updated_at = completed_at;
        turn_id
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(key).cloned()
    }
}
