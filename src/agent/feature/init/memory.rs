//! Stable initialization helpers for `Feature::Memory`.

use crate::agent::MemoryRepository;
use anyhow::Result;
use tracing::{debug, info};

const MEMORY_TOOLSET_NAME: &str = "memory";

/// Return the toolsets owned by the memory feature during initialization.
pub fn toolsets() -> Vec<String> {
    vec![MEMORY_TOOLSET_NAME.to_string()]
}

/// Build the stable memory usage prompt from the active memory catalog.
pub fn usage(thread_id: &str, repository: &MemoryRepository) -> Result<Option<String>> {
    debug!(
        thread_id,
        root = %repository.memory_root().display(),
        "starting memory feature usage prompt build"
    );
    let prompt = repository.active_catalog_prompt()?;
    if prompt.is_some() {
        info!(
            thread_id,
            root = %repository.memory_root().display(),
            "built memory feature usage prompt"
        );
    } else {
        info!(
            thread_id,
            root = %repository.memory_root().display(),
            "memory feature enabled but no active catalog prompt is available"
        );
    }
    Ok(prompt)
}
