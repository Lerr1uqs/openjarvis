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
        Self::with_id(
            Uuid::new_v4(),
            external_message_id,
            messages,
            started_at,
            completed_at,
        )
    }

    fn with_id(
        id: Uuid,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
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
    pub id: Uuid,
    pub external_thread_id: String,
    pub turns: Vec<ConversationTurn>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationThread {
    /// Create an empty thread with a generated internal id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let thread = ConversationThread::new("default", Utc::now());
    /// assert_eq!(thread.external_thread_id, "default");
    /// ```
    pub fn new(external_thread_id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self::with_id(Uuid::new_v4(), external_thread_id, now)
    }

    /// Create an empty thread with an explicit internal id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    /// use uuid::Uuid;
    ///
    /// let thread_id = Uuid::new_v4();
    /// let thread = ConversationThread::with_id(thread_id, "default", Utc::now());
    /// assert_eq!(thread.id, thread_id);
    /// ```
    pub fn with_id(id: Uuid, external_thread_id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            id,
            external_thread_id: external_thread_id.into(),
            turns: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Load the turn for the incoming external message id or create it on first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    /// use uuid::Uuid;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// let turn_id = Uuid::new_v4();
    ///
    /// let first_id = thread
    ///     .load_or_create_turn(Some("msg_1".to_string()), turn_id, now, now)
    ///     .id;
    /// let second_id = thread
    ///     .load_or_create_turn(Some("msg_1".to_string()), Uuid::new_v4(), now, now)
    ///     .id;
    ///
    /// assert_eq!(first_id, second_id);
    /// ```
    pub fn load_or_create_turn(
        &mut self,
        external_message_id: Option<String>,
        turn_id: Uuid,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> &mut ConversationTurn {
        if let Some(message_id) = external_message_id.as_deref() {
            if let Some(index) = self
                .turns
                .iter()
                .position(|turn| turn.external_message_id.as_deref() == Some(message_id))
            {
                let turn = &mut self.turns[index];
                turn.started_at = started_at;
                turn.completed_at = completed_at;
                return turn;
            }
        }

        self.turns.push(ConversationTurn::with_id(
            turn_id,
            external_message_id,
            Vec::new(),
            started_at,
            completed_at,
        ));
        self.updated_at = completed_at;
        self.turns
            .last_mut()
            .expect("turn should exist immediately after insertion")
    }

    /// Store one normalized turn payload into the thread, creating the turn on first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// let turn_id = thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// assert_eq!(thread.turns[0].id, turn_id);
    /// ```
    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        let turn_id = {
            let turn = self.load_or_create_turn(
                external_message_id,
                Uuid::new_v4(),
                started_at,
                completed_at,
            );
            turn.messages = messages;
            turn.started_at = started_at;
            turn.completed_at = completed_at;
            turn.id
        };
        self.updated_at = completed_at;
        turn_id
    }

    /// Retain only the latest `max_messages` across the whole thread.
    ///
    /// Empty turns left behind by trimming are removed so the stored thread shape converges with
    /// the retained history window.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_0", now),
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_1", now),
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_2", now),
    ///     ],
    ///     now,
    ///     now,
    /// );
    ///
    /// thread.retain_latest_messages(2);
    ///
    /// assert_eq!(
    ///     thread
    ///         .load_messages()
    ///         .into_iter()
    ///         .map(|message| message.content)
    ///         .collect::<Vec<_>>(),
    ///     vec!["message_1".to_string(), "message_2".to_string()]
    /// );
    /// ```
    pub fn retain_latest_messages(&mut self, max_messages: usize) {
        if max_messages == 0 {
            self.turns.clear();
            return;
        }

        let mut remaining_drop = self
            .turns
            .iter()
            .map(|turn| turn.messages.len())
            .sum::<usize>()
            .saturating_sub(max_messages);

        if remaining_drop == 0 {
            return;
        }

        for turn in &mut self.turns {
            if remaining_drop == 0 {
                break;
            }

            let turn_drop = remaining_drop.min(turn.messages.len());
            if turn_drop > 0 {
                turn.messages.drain(0..turn_drop);
                remaining_drop -= turn_drop;
            }
        }

        self.turns.retain(|turn| !turn.messages.is_empty());
    }

    /// Load the flattened message history for the whole thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.turns
            .iter()
            .flat_map(|turn| turn.messages.iter().cloned())
            .collect()
    }
}
