//! Conversation-thread persistence types used by the session manager.

use crate::context::{ChatMessage, ChatMessageRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

impl ConversationTurn {
    /// Create a new stored turn from normalized chat messages.
    pub fn new(
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            external_message_id,
            messages,
            started_at,
            completed_at,
        }
    }

    /// Return the final assistant-visible message recorded in this turn.
    pub fn final_assistant_message(&self) -> Option<&ChatMessage> {
        self.messages.iter().rev().find(|message| {
            message.role == ChatMessageRole::Assistant && !message.content.trim().is_empty()
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationThread {
    pub id: String,
    pub turns: Vec<ConversationTurn>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationThread {
    /// Create an empty thread with the provided identifier.
    pub fn new(id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            id: id.into(),
            turns: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        let turn = ConversationTurn::new(external_message_id, messages, started_at, completed_at);
        let turn_id = turn.id;
        self.turns.push(turn);
        self.updated_at = completed_at;
        turn_id
    }

    /// Load the flattened message history for the whole thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.turns
            .iter()
            .flat_map(|turn| turn.messages.iter().cloned())
            .collect()
    }
}
