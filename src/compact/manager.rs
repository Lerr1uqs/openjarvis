//! Compact manager that coordinates plan selection, provider calls, and replacement-turn creation.

use crate::{
    compact::{
        COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactProvider,
        CompactRequest, CompactStrategy, CompactSummary, CompactionPlan,
    },
    context::{ChatMessage, ChatMessageRole},
    thread::{ConversationThread, ConversationTurn},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::info;

/// Result of applying one compact operation to a thread.
#[derive(Debug, Clone)]
pub struct CompactionOutcome {
    pub strategy_name: String,
    pub plan: CompactionPlan,
    pub summary: CompactSummary,
    pub compacted_turn: ConversationTurn,
    pub compacted_thread: ConversationThread,
}

/// Standalone compact manager that is ready to be wired into runtime later.
pub struct CompactManager {
    provider: Arc<dyn CompactProvider>,
    strategy: Arc<dyn CompactStrategy>,
}

impl CompactManager {
    /// Create one compact manager from the selected provider and strategy.
    pub fn new(provider: Arc<dyn CompactProvider>, strategy: Arc<dyn CompactStrategy>) -> Self {
        Self { provider, strategy }
    }

    /// Compact one thread and return the fully materialized replacement result.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::{
    ///         CompactAllChatStrategy, CompactManager, CompactSummary, StaticCompactProvider,
    ///     },
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::ConversationThread,
    /// };
    /// use std::sync::Arc;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    /// let manager = CompactManager::new(
    ///     Arc::new(StaticCompactProvider::new(CompactSummary {
    ///         compacted_assistant: "压缩后的上下文".to_string(),
    ///     })),
    ///     Arc::new(CompactAllChatStrategy),
    /// );
    ///
    /// let outcome = manager.compact_thread(&thread, now).await?;
    /// assert!(outcome.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn compact_thread(
        &self,
        thread: &ConversationThread,
        compacted_at: DateTime<Utc>,
    ) -> Result<Option<CompactionOutcome>> {
        let Some(plan) = self.strategy.build_plan(thread)? else {
            return Ok(None);
        };
        let source_messages = plan.source_messages(thread)?;
        let request = CompactRequest::new(plan.source_turn_ids.clone(), source_messages)?;

        info!(
            strategy = self.strategy.name(),
            source_turn_count = request.source_turn_ids.len(),
            source_message_count = request.messages.len(),
            "starting compact manager run"
        );

        let summary = self.provider.compact(request).await?;
        let compacted_turn = build_compacted_turn(&summary, compacted_at);
        let compacted_thread = plan.apply(thread, compacted_turn.clone())?;

        Ok(Some(CompactionOutcome {
            strategy_name: self.strategy.name().to_string(),
            plan,
            summary,
            compacted_turn,
            compacted_thread,
        }))
    }
}

/// Build the persisted replacement turn that stands in for compacted source history.
pub fn build_compacted_turn(
    summary: &CompactSummary,
    compacted_at: DateTime<Utc>,
) -> ConversationTurn {
    let compacted_messages = vec![
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
    ];

    ConversationTurn::new(None, compacted_messages, compacted_at, compacted_at)
}
