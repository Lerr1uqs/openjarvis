//! In-memory session storage that groups conversation threads by channel and user.
//!
//! This store is responsible for session state, not execution scheduling. Today callers are
//! expected to feed turns in order. When concurrent turn execution is introduced later, ordering
//! should be enforced by the agent-side inbox or worker scheduler rather than by this store alone.

use crate::context::{ChatMessage, ChatMessageRole, MessageContext};
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
pub struct PendingTurn {
    pub session_key: SessionKey,
    pub thread_id: String,
    pub turn_id: Uuid,
    pub context: MessageContext,
}

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionKey, Session>>,
}

impl SessionManager {
    /// Create an empty in-memory session manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new turn, creating the session and thread on demand and materializing the next context.
    ///
    /// This method only mutates session state for the new pending turn. It does not by itself
    /// prevent two turns from the same session or thread from running concurrently.
    pub async fn begin_turn(&self, incoming: &IncomingMessage, system_prompt: &str) -> PendingTurn {
        let now = Utc::now();
        let session_key = SessionKey::from_incoming(incoming);
        let thread_id = resolve_thread_id(incoming);
        let received_at = incoming.received_at;
        let external_message_id = incoming.external_message_id.clone();
        let user_message = incoming.content.clone();

        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::new(now));
        let thread = session
            .threads
            .entry(thread_id.clone())
            .or_insert_with(|| ConversationThread::new(thread_id.clone(), now));
        let turn_id = thread.append_user_turn(external_message_id, user_message, received_at);
        session.updated_at = now;

        let mut context = MessageContext::with_system_prompt(system_prompt.to_string());
        context.extend_from_thread(thread);

        PendingTurn {
            session_key,
            thread_id,
            turn_id,
            context,
        }
    }

    /// Complete a turn with a plain assistant reply.
    pub async fn complete_turn(&self, pending: &PendingTurn, assistant_message: &str) {
        let completed_at = Utc::now();
        self.complete_turn_with_messages(
            pending,
            vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                assistant_message,
                completed_at,
            )],
            completed_at,
        )
        .await;
    }

    pub async fn complete_turn_with_messages(
        &self,
        pending: &PendingTurn,
        messages: Vec<ChatMessage>,
        completed_at: DateTime<Utc>,
    ) {
        // Complete the pending turn after the agent loop finishes.
        //
        // This write is intentionally narrow. Any future concurrent execution model still needs
        // an external ordering guarantee so turns from the same session or thread are completed
        // in the intended sequence.
        let mut sessions = self.sessions.write().await;
        let Some(session) = sessions.get_mut(&pending.session_key) else {
            return;
        };
        let Some(thread) = session.threads.get_mut(&pending.thread_id) else {
            return;
        };

        thread.complete_turn_with_messages(pending.turn_id, messages, completed_at);
        session.updated_at = completed_at;
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(key).cloned()
    }
}

fn resolve_thread_id(incoming: &IncomingMessage) -> String {
    // Use the upstream thread id when present, otherwise keep a single default thread.
    incoming
        .thread_id
        .clone()
        .filter(|thread_id| !thread_id.trim().is_empty())
        .unwrap_or_else(|| "default".to_string())
}
