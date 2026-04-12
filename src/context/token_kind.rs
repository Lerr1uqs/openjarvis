//! Context token bucket kinds shared by budget estimation and runtime prompts.

use super::ChatMessageRole;
use serde::{Deserialize, Serialize};

/// Stable token-bucket kinds used by context budget estimation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ContextTokenKind {
    #[serde(rename = "system_tokens")]
    System,
    #[serde(rename = "chat_tokens")]
    Chat,
    #[serde(rename = "visible_tool_tokens")]
    VisibleTool,
    #[serde(rename = "reserved_output_tokens")]
    ReservedOutput,
}

impl ContextTokenKind {
    /// Stable ordered list of token buckets used in request-level budget reporting.
    pub const ALL: [Self; 4] = [
        Self::System,
        Self::Chat,
        Self::VisibleTool,
        Self::ReservedOutput,
    ];

    /// Return the stable field label used in budget payloads and prompt rendering.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::context::ContextTokenKind;
    ///
    /// assert_eq!(ContextTokenKind::VisibleTool.as_str(), "visible_tool_tokens");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system_tokens",
            Self::Chat => "chat_tokens",
            Self::VisibleTool => "visible_tool_tokens",
            Self::ReservedOutput => "reserved_output_tokens",
        }
    }

    /// Map one chat message role into its request-budget token bucket.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::context::{ChatMessageRole, ContextTokenKind};
    ///
    /// assert_eq!(
    ///     ContextTokenKind::for_chat_message_role(&ChatMessageRole::ToolResult),
    ///     ContextTokenKind::Chat
    /// );
    /// ```
    pub fn for_chat_message_role(role: &ChatMessageRole) -> Self {
        match role {
            ChatMessageRole::System => Self::System,
            ChatMessageRole::User
            | ChatMessageRole::Assistant
            | ChatMessageRole::Reasoning
            | ChatMessageRole::Toolcall
            | ChatMessageRole::ToolResult => Self::Chat,
        }
    }
}
