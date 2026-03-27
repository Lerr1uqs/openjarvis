//! Thread-scoped compact primitives.
//!
//! This module is intentionally standalone for the first review pass. It can compact a
//! `ConversationThread` into one replacement turn without being wired into the agent runtime yet.

pub mod budget;
pub mod manager;
pub mod provider;
pub mod runtime;
pub mod strategy;

use serde::{Deserialize, Serialize};

pub use budget::{CHARS_DIV4_TOKENIZER, ContextBudgetEstimator, ContextBudgetReport};
pub use manager::{CompactManager, CompactionOutcome, build_compacted_turn};
pub use provider::{
    COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactPrompt, CompactProvider,
    CompactRequest, CompactSummary, LLMCompactProvider, StaticCompactProvider,
    build_compact_prompt, render_chat_history,
};
pub use runtime::{CompactRuntimeManager, CompactScopeKey};
pub use strategy::{CompactAllChatStrategy, CompactStrategy, CompactionPlan};

/// Describe how the selected source turns should be handled after compaction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactSourceHandling {
    /// Remove the original source turns from active history after the compacted turn is written.
    DropSource,
    /// Move the original source turns into a separate archived store instead of keeping them active.
    ArchiveSource,
    /// Keep a hidden copy of the original source turns alongside the compacted replacement turn.
    KeepShadowCopy,
}
