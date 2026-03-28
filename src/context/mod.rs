//! Context and chat message types used to assemble prompt history for the agent loop.

use crate::thread::ConversationThread;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatMessageRole {
    System,
    Memory,
    User,
    Assistant,
    Toolcall,
    ToolResult,
}

impl ChatMessageRole {
    /// Return the stable label used when rendering messages into plain-text prompts.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Memory => "memory",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Toolcall => "toolcall",
            Self::ToolResult => "tool_result",
        }
    }
}

pub mod token_kind;

pub use token_kind::ContextTokenKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ChatMessage {
    /// Create a structured chat message with empty tool metadata.
    pub fn new(
        role: ChatMessageRole,
        content: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            created_at,
        }
    }

    /// Attach assistant-side tool calls to the message.
    pub fn with_tool_calls(mut self, tool_calls: Vec<ChatToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Attach the originating `tool_call_id` to a tool result message.
    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }
}

pub type Messages = Vec<ChatMessage>;
pub type ContextMessage = MessageContext;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageContext {
    pub system: Vec<ChatMessage>,
    pub memory: Vec<ChatMessage>,
    pub chat: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub system_prompt: String,
    pub user_message: String,
}

impl MessageContext {
    /// Create a context initialized with one system prompt message.
    pub fn with_system_prompt(system_prompt: impl Into<String>) -> Self {
        let mut context = Self::default();
        context.push_system(system_prompt);
        context
    }

    /// Append a system message to the context.
    pub fn push_system(&mut self, content: impl Into<String>) {
        self.system.push(ChatMessage::new(
            ChatMessageRole::System,
            content,
            Utc::now(),
        ));
    }

    #[allow(dead_code)]
    /// Append a memory message to the context.
    pub fn push_memory(&mut self, content: impl Into<String>) {
        self.memory.push(ChatMessage::new(
            ChatMessageRole::Memory,
            content,
            Utc::now(),
        ));
    }

    /// Append chat messages that were already normalized to the unified protocol shape.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole, MessageContext};
    ///
    /// let mut context = MessageContext::with_system_prompt("system");
    /// context.extend_chat_messages(vec![ChatMessage::new(
    ///     ChatMessageRole::User,
    ///     "hello",
    ///     Utc::now(),
    /// )]);
    ///
    /// assert_eq!(context.chat.len(), 1);
    /// ```
    pub fn extend_chat_messages<I>(&mut self, messages: I)
    where
        I: IntoIterator<Item = ChatMessage>,
    {
        self.chat.extend(messages);
    }

    /// Extend chat history from an existing conversation thread.
    pub fn extend_from_thread(&mut self, thread: &ConversationThread) {
        self.extend_chat_messages(thread.load_messages());
    }

    /// Return a read-only-style copy of the context messages in prompt order.
    pub fn as_messages(&self) -> Messages {
        let mut messages =
            Vec::with_capacity(self.system.len() + self.memory.len() + self.chat.len());
        messages.extend(self.system.iter().cloned());
        messages.extend(self.memory.iter().cloned());
        messages.extend(self.chat.iter().cloned());
        messages
    }

    /// Render the context into the simplified prompt shape used by compatibility helpers.
    pub fn render_for_llm(&self) -> RenderedPrompt {
        let mut system_sections: Vec<String> =
            self.system.iter().map(|msg| msg.content.clone()).collect();
        if !self.memory.is_empty() {
            let memory_section = self
                .memory
                .iter()
                .map(|msg| format!("- {}", msg.content))
                .collect::<Vec<_>>()
                .join("\n");
            system_sections.push(format!("Memory:\n{memory_section}"));
        }

        let user_message = self
            .chat
            .iter()
            .map(|msg| format!("{}: {}", msg.role.as_label(), msg.content))
            .collect::<Vec<_>>()
            .join("\n");

        RenderedPrompt {
            system_prompt: system_sections.join("\n\n"),
            user_message,
        }
    }
}
