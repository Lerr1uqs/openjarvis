//! Thread-init feature provider that injects the stable active-memory catalog snapshot.

use super::repository::MemoryRepository;
use crate::{
    agent::feature::{FeaturePromptBuildContext, FeaturePromptProvider},
    context::{ChatMessage, ChatMessageRole},
};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

/// Build one stable active-memory catalog prompt from the local repository.
pub struct ActiveMemoryCatalogFeaturePromptProvider {
    repository: Arc<MemoryRepository>,
}

impl ActiveMemoryCatalogFeaturePromptProvider {
    /// Create the provider from one shared memory repository.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use openjarvis::agent::memory::{
    ///     ActiveMemoryCatalogFeaturePromptProvider, MemoryRepository,
    /// };
    ///
    /// let repository = Arc::new(MemoryRepository::new("/tmp/openjarvis-workspace"));
    /// let _provider = ActiveMemoryCatalogFeaturePromptProvider::new(repository);
    /// ```
    pub fn new(repository: Arc<MemoryRepository>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl FeaturePromptProvider for ActiveMemoryCatalogFeaturePromptProvider {
    fn name(&self) -> &'static str {
        "active_memory_catalog"
    }

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>> {
        debug!(
            thread_id = %context.thread_context.locator.thread_id,
            root = %self.repository.memory_root().display(),
            "starting active memory feature prompt build"
        );
        let prompt = self.repository.active_catalog_prompt()?;
        let Some(prompt) = prompt else {
            info!(
                thread_id = %context.thread_context.locator.thread_id,
                "no active memory catalog available for thread init"
            );
            return Ok(Vec::new());
        };

        info!(
            thread_id = %context.thread_context.locator.thread_id,
            root = %self.repository.memory_root().display(),
            "built active memory catalog system prompt"
        );
        debug!(
            thread_id = %context.thread_context.locator.thread_id,
            prompt_len = prompt.len(),
            "completed active memory feature prompt build"
        );
        Ok(vec![ChatMessage::new(
            ChatMessageRole::System,
            prompt,
            context.created_at,
        )])
    }
}
