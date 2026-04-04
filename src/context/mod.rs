//! Unified chat message protocol shared by thread persistence and LLM requests.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatMessageRole {
    System,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
