//! Stable initialization helpers for `Feature::Skill`.

use crate::agent::ToolRegistry;
use tracing::info;

/// Return whether the always-visible tool belongs to the skill feature.
pub fn owns_always_visible_tool(tool_name: &str) -> bool {
    tool_name == "load_skill"
}

/// Build the stable skill catalog prompt used during thread initialization.
pub async fn usage(thread_id: &str, tool_registry: &ToolRegistry) -> Option<String> {
    let prompt = tool_registry.skills().catalog_prompt().await;
    if prompt.is_some() {
        info!(thread_id, "built skill feature usage prompt");
    } else {
        info!(
            thread_id,
            "skill feature enabled but no enabled local skill catalog is available"
        );
    }
    prompt
}
