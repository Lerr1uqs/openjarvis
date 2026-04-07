//! Compact manager that summarizes one message sequence into replacement messages.

use crate::{
    compact::{
        COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactProvider,
        CompactRequest, CompactSummary,
    },
    context::{ChatMessage, ChatMessageRole},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::info;

/// Result of compacting one message sequence directly.
#[derive(Debug, Clone)]
pub struct MessageCompactionOutcome {
    pub source_message_count: usize,
    pub summary: CompactSummary,
    pub compacted_messages: Vec<ChatMessage>,
}

/// Standalone compact manager that only depends on a provider and message input.
pub struct CompactManager {
    provider: Arc<dyn CompactProvider>,
}

impl CompactManager {
    /// Create one compact manager from the selected provider.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::{CompactManager, CompactSummary, StaticCompactProvider},
    ///     context::{ChatMessage, ChatMessageRole},
    /// };
    /// use std::sync::Arc;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let manager = CompactManager::new(Arc::new(StaticCompactProvider::new(CompactSummary {
    ///     compacted_assistant: "压缩后的上下文".to_string(),
    /// })));
    /// let messages = vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())];
    ///
    /// let outcome = manager.compact_messages(&messages, Utc::now()).await?;
    /// assert!(outcome.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(provider: Arc<dyn CompactProvider>) -> Self {
        Self { provider }
    }

    /// Compact one message sequence and return the replacement messages.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::{CompactManager, CompactSummary, StaticCompactProvider},
    ///     context::{ChatMessage, ChatMessageRole},
    /// };
    /// use std::sync::Arc;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let manager = CompactManager::new(Arc::new(StaticCompactProvider::new(CompactSummary {
    ///     compacted_assistant: "压缩后的上下文".to_string(),
    /// })));
    /// let messages = vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())];
    ///
    /// let outcome = manager.compact_messages(&messages, Utc::now()).await?;
    /// assert_eq!(outcome.expect("should compact").compacted_messages.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn compact_messages(
        &self,
        messages: &[ChatMessage],
        compacted_at: DateTime<Utc>,
    ) -> Result<Option<MessageCompactionOutcome>> {
        if messages.is_empty() {
            return Ok(None);
        }

        let compactable_messages = messages
            .iter()
            .filter(|message| message.role != ChatMessageRole::System)
            .cloned()
            .collect::<Vec<_>>();
        if compactable_messages.is_empty() {
            return Ok(None);
        }

        let request = CompactRequest::new(compactable_messages.clone())?;
        info!(
            source_message_count = request.messages.len(),
            "starting compact manager run from messages"
        );

        let summary = self.provider.compact(request).await?;
        let compacted_messages = build_compacted_messages(&summary, compacted_at);

        Ok(Some(MessageCompactionOutcome {
            source_message_count: compactable_messages.len(),
            summary,
            compacted_messages,
        }))
    }
}

/// Build the persisted replacement messages that stand in for compacted source history.
pub fn build_compacted_messages(
    summary: &CompactSummary,
    compacted_at: DateTime<Utc>,
) -> Vec<ChatMessage> {
    vec![
        ChatMessage::new(
            ChatMessageRole::Assistant,
            format!(
                "{}{}",
                COMPACTED_ASSISTANT_PREFIX, summary.compacted_assistant
            ),
            compacted_at,
        ),
        ChatMessage::new(
            ChatMessageRole::User,
            COMPACTED_USER_CONTINUE_MESSAGE,
            compacted_at,
        ),
    ]
}
