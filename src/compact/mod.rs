//! Thread-scoped compact primitives.
//!
//! The compact boundary is message-based: callers pass the non-system messages that should be
//! summarized, and the compact manager returns replacement messages ready to be written back to
//! `Thread`.

pub mod budget;
pub mod manager;
pub mod provider;

pub use budget::{CHARS_DIV4_TOKENIZER, ContextBudgetEstimator, ContextBudgetReport};
pub use manager::{CompactManager, MessageCompactionOutcome, build_compacted_messages};
pub use provider::{
    COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactPrompt, CompactProvider,
    CompactRequest, CompactSummary, LLMCompactProvider, StaticCompactProvider,
    build_compact_prompt, render_chat_history,
};
