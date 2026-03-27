//! Compaction planning strategies.

use crate::{
    compact::CompactSourceHandling,
    context::ChatMessage,
    thread::{ConversationThread, ConversationTurn},
};
use anyhow::{Result, anyhow, bail};
use std::collections::HashSet;
use tracing::info;
use uuid::Uuid;

/// One executable compaction plan describing which turns should be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPlan {
    pub source_turn_ids: Vec<Uuid>,
    pub source_handling: CompactSourceHandling,
}

impl CompactionPlan {
    /// Create one validated compaction plan.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::{CompactSourceHandling, CompactionPlan},
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::ConversationThread,
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    /// let plan = CompactionPlan::new(
    ///     vec![thread.turns[0].id],
    ///     CompactSourceHandling::DropSource,
    /// );
    ///
    /// assert!(plan.is_ok());
    /// ```
    pub fn new(source_turn_ids: Vec<Uuid>, source_handling: CompactSourceHandling) -> Result<Self> {
        if source_turn_ids.is_empty() {
            bail!("compaction plan must contain at least one source turn id");
        }

        let unique_ids = source_turn_ids.iter().copied().collect::<HashSet<_>>();
        if unique_ids.len() != source_turn_ids.len() {
            bail!("compaction plan contains duplicate source turn ids");
        }

        Ok(Self {
            source_turn_ids,
            source_handling,
        })
    }

    /// Return the source turns in thread order.
    pub fn source_turns<'a>(
        &self,
        thread: &'a ConversationThread,
    ) -> Result<Vec<&'a ConversationTurn>> {
        Ok(self
            .locate_turn_positions(thread)?
            .into_iter()
            .map(|(_, turn)| turn)
            .collect())
    }

    /// Return the flattened source chat messages that will be summarized.
    pub fn source_messages(&self, thread: &ConversationThread) -> Result<Vec<ChatMessage>> {
        Ok(self
            .source_turns(thread)?
            .into_iter()
            .flat_map(|turn| turn.messages.iter().cloned())
            .collect())
    }

    /// Replace the selected source turns with one replacement turn and return the updated thread.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::{CompactSourceHandling, CompactionPlan},
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ConversationThread, ConversationTurn},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    /// let source_turn_id = thread.turns[0].id;
    /// let plan = CompactionPlan::new(vec![source_turn_id], CompactSourceHandling::DropSource)
    ///     .expect("plan should build");
    /// let compacted_turn = ConversationTurn::new(
    ///     None,
    ///     vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
    ///         ChatMessage::new(ChatMessageRole::User, "继续", now),
    ///     ],
    ///     now,
    ///     now,
    /// );
    ///
    /// let compacted = plan.apply(&thread, compacted_turn).expect("plan should apply");
    /// assert_eq!(compacted.turns.len(), 1);
    /// assert_eq!(compacted.turns[0].messages.len(), 2);
    /// ```
    pub fn apply(
        &self,
        thread: &ConversationThread,
        replacement_turn: ConversationTurn,
    ) -> Result<ConversationThread> {
        let mut compacted_thread = thread.clone();
        self.apply_in_place(&mut compacted_thread, replacement_turn)?;
        Ok(compacted_thread)
    }

    /// Replace the selected source turns with one replacement turn in place.
    pub fn apply_in_place(
        &self,
        thread: &mut ConversationThread,
        replacement_turn: ConversationTurn,
    ) -> Result<()> {
        let positions = self.locate_turn_positions(thread)?;
        let indexes = positions
            .into_iter()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        ensure_contiguous_indexes(&indexes)?;

        let first_index = *indexes
            .first()
            .ok_or_else(|| anyhow!("compaction plan did not resolve any source turns"))?;
        let last_index = *indexes
            .last()
            .ok_or_else(|| anyhow!("compaction plan did not resolve any source turns"))?;

        info!(
            source_turn_count = indexes.len(),
            insert_index = first_index,
            source_handling = ?self.source_handling,
            "applying compaction plan to thread"
        );

        let completed_at = replacement_turn.completed_at;
        thread
            .turns
            .splice(first_index..=last_index, std::iter::once(replacement_turn));
        thread.updated_at = completed_at;
        Ok(())
    }

    fn locate_turn_positions<'a>(
        &self,
        thread: &'a ConversationThread,
    ) -> Result<Vec<(usize, &'a ConversationTurn)>> {
        let wanted_ids = self.source_turn_ids.iter().copied().collect::<HashSet<_>>();
        let resolved = thread
            .turns
            .iter()
            .enumerate()
            .filter(|(_, turn)| wanted_ids.contains(&turn.id))
            .collect::<Vec<_>>();

        if resolved.len() != self.source_turn_ids.len() {
            let missing_turn_ids = self
                .source_turn_ids
                .iter()
                .filter(|turn_id| !resolved.iter().any(|(_, turn)| turn.id == **turn_id))
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            bail!(
                "compaction plan references unknown source turns: {}",
                missing_turn_ids.join(", ")
            );
        }

        Ok(resolved)
    }
}

fn ensure_contiguous_indexes(indexes: &[usize]) -> Result<()> {
    for window in indexes.windows(2) {
        if window[1] != window[0] + 1 {
            bail!("compaction plan source turns must form one contiguous slice");
        }
    }

    Ok(())
}

/// Strategy abstraction that selects which active chat turns should be compacted.
pub trait CompactStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn build_plan(&self, thread: &ConversationThread) -> Result<Option<CompactionPlan>>;
}

/// First compact strategy that replaces the whole active chat history.
#[derive(Debug, Default, Clone, Copy)]
pub struct CompactAllChatStrategy;

impl CompactStrategy for CompactAllChatStrategy {
    fn name(&self) -> &'static str {
        "compact_all_chat"
    }

    fn build_plan(&self, thread: &ConversationThread) -> Result<Option<CompactionPlan>> {
        let source_turn_ids = thread
            .turns
            .iter()
            .filter(|turn| !turn.messages.is_empty())
            .map(|turn| turn.id)
            .collect::<Vec<_>>();
        if source_turn_ids.is_empty() {
            return Ok(None);
        }

        info!(
            strategy = self.name(),
            source_turn_count = source_turn_ids.len(),
            source_message_count = thread.load_messages().len(),
            "built compact-all-chat plan"
        );

        CompactionPlan::new(source_turn_ids, CompactSourceHandling::DropSource).map(Some)
    }
}
