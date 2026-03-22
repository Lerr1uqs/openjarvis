//! Conversation-thread persistence types used by the session manager.

use crate::context::{ChatMessage, ChatMessageRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    pub user_message: String,
    pub assistant_message: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ConversationTurn {
    /// Create a new turn that starts with the user's message.
    pub fn new(
        external_message_id: Option<String>,
        user_message: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        let user_message = user_message.into();
        Self {
            id: Uuid::new_v4(),
            external_message_id,
            user_message: user_message.clone(),
            assistant_message: None,
            messages: vec![ChatMessage::new(
                ChatMessageRole::User,
                user_message,
                started_at,
            )],
            started_at,
            completed_at: None,
        }
    }

    /// Complete the turn with a plain assistant message.
    pub fn complete(&mut self, assistant_message: impl Into<String>, completed_at: DateTime<Utc>) {
        let assistant_message = assistant_message.into();
        self.complete_with_messages(
            vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                assistant_message,
                completed_at,
            )],
            completed_at,
        );
    }

    pub fn complete_with_messages(
        &mut self,
        messages: Vec<ChatMessage>,
        completed_at: DateTime<Utc>,
    ) {
        self.assistant_message = select_final_assistant_message(&messages);
        self.messages.extend(messages);
        self.completed_at = Some(completed_at);
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

    pub fn append_user_turn(
        &mut self,
        external_message_id: Option<String>,
        user_message: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Uuid {
        let turn = ConversationTurn::new(external_message_id, user_message, started_at);
        let turn_id = turn.id;
        self.turns.push(turn);
        self.updated_at = started_at;
        turn_id
    }

    pub fn complete_turn(
        &mut self,
        turn_id: Uuid,
        assistant_message: impl Into<String>,
        completed_at: DateTime<Utc>,
    ) -> bool {
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == turn_id) else {
            return false;
        };

        turn.complete(assistant_message, completed_at);
        self.updated_at = completed_at;
        true
    }

    pub fn complete_turn_with_messages(
        &mut self,
        turn_id: Uuid,
        messages: Vec<ChatMessage>,
        completed_at: DateTime<Utc>,
    ) -> bool {
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == turn_id) else {
            return false;
        };

        turn.complete_with_messages(messages, completed_at);
        self.updated_at = completed_at;
        true
    }
}

fn select_final_assistant_message(messages: &[ChatMessage]) -> Option<String> {
    // Prefer the last plain assistant reply, but fall back to the most recent assistant message.
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role == ChatMessageRole::Assistant
                && message.tool_calls.is_empty()
                && !message.content.trim().is_empty()
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .find(|message| message.role == ChatMessageRole::Assistant)
        })
        .map(|message| message.content.clone())
}
