#![allow(dead_code)]

use anyhow::Result;
use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole},
    thread::{Thread, ThreadToolEvent},
};

pub trait ThreadTestExt {
    fn non_system_messages(&self) -> Vec<ChatMessage>;
    fn system_messages(&self) -> Vec<ChatMessage>;
    fn seed_persisted_messages(&mut self, messages: Vec<ChatMessage>);
    fn append_persisted_messages_for_test(&mut self, messages: Vec<ChatMessage>);
    fn append_persisted_messages_with_state_for_test(
        &mut self,
        messages: Vec<ChatMessage>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
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
        tool_events: Vec<ThreadToolEvent>,
    ) {
        self.replace_loaded_toolsets(loaded_toolsets);
        self.state.tools.tool_events.extend(tool_events);
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
