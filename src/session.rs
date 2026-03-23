//! In-memory session storage keyed by channel, user, and thread.
//!
//! Router owns the session store and uses `load_turn`/`store_turn` to bridge external incoming
//! messages with the agent's normalized chat history. The stored payload is the unified chat
//! protocol message list so providers such as OpenAI and Anthropic can share one persistence
//! layer while keeping provider-specific serialization in the LLM layer.

use crate::context::ChatMessage;
use crate::model::IncomingMessage;
use crate::thread::ConversationThread;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;
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
    pub channel: String,
    pub user_id: String,
    pub thread_id: String,
}

impl ThreadLocator {
    /// Build a stable session-thread locator from a normalized incoming message.
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
    /// let locator = ThreadLocator::from_incoming(&incoming);
    /// assert_eq!(locator.thread_id, "default");
    /// ```
    pub fn from_incoming(incoming: &IncomingMessage) -> Self {
        Self {
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
            thread_id: incoming.resolved_thread_id(),
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
    pub threads: HashMap<String, ConversationThread>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create an empty session snapshot.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            threads: HashMap::new(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionStrategy {
    pub max_messages_per_turn: usize,
}

impl Default for SessionStrategy {
    fn default() -> Self {
        Self {
            max_messages_per_turn: 5,
        }
    }
}

impl SessionStrategy {
    /// Apply the current turn-storage policy to one normalized turn message list.
    pub fn retain_messages(&self, mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        if self.max_messages_per_turn == 0 {
            return Vec::new();
        }

        let drop_count = messages.len().saturating_sub(self.max_messages_per_turn);
        if drop_count > 0 {
            messages.drain(0..drop_count);
        }
        messages
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
    /// assert_eq!(manager.strategy().max_messages_per_turn, 5);
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

    /// Load the current flattened history for one channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     thread_id: "default".to_string(),
    /// };
    ///
    /// let _future = manager.load_turn(&locator);
    /// ```
    pub async fn load_turn(&self, locator: &ThreadLocator) -> Vec<ChatMessage> {
        let sessions = self.sessions.read().await;
        sessions
            .get(&locator.session_key())
            .and_then(|session| session.threads.get(&locator.thread_id))
            .map(ConversationThread::load_messages)
            .unwrap_or_default()
    }

    /// Store one completed turn for the provided channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     thread_id: "default".to_string(),
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
        let session_key = locator.session_key();
        let retained_messages = self.strategy.retain_messages(messages);
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key)
            .or_insert_with(|| Session::new(completed_at));
        let thread = session
            .threads
            .entry(locator.thread_id.clone())
            .or_insert_with(|| ConversationThread::new(locator.thread_id.clone(), completed_at));
        let turn_id = thread.store_turn(
            external_message_id,
            retained_messages,
            started_at,
            completed_at,
        );
        session.updated_at = completed_at;
        turn_id
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(key).cloned()
    }
}
