//! Stable initialization helpers for the `obswiki` child-thread profile.

use crate::agent::ToolRegistry;
use anyhow::{Result, anyhow};
use tracing::{debug, info};

/// Build the stable `obswiki` vault context prompts injected into the child-thread system prefix.
pub async fn usage(tool_registry: &ToolRegistry) -> Result<Vec<String>> {
    debug!("building obswiki child-thread initialization prompts");
    let runtime = tool_registry
        .obswiki_runtime()
        .await
        .ok_or_else(|| anyhow!("obswiki runtime is not registered"))?;
    let context = runtime.load_vault_context().await?;
    let preflight = &context.preflight;
    let prompts = vec![
        format!(
            concat!(
                "Obswiki vault runtime status:\n",
                "- vault_path: {}\n",
                "- obsidian_cli_available: {}\n",
                "- qmd_configured: {}\n",
                "- qmd_cli_available: {}\n",
                "- raw_dir_exists: {}\n",
                "- wiki_dir_exists: {}\n",
                "- schema_dir_exists: {}\n",
                "- index_file_exists: {}\n",
                "- agents_file_exists: {}"
            ),
            preflight.vault_path.display(),
            preflight.obsidian_cli_available,
            preflight.qmd_configured,
            preflight.qmd_cli_available,
            preflight.raw_dir_exists,
            preflight.wiki_dir_exists,
            preflight.schema_dir_exists,
            preflight.index_file_exists,
            preflight.agents_file_exists,
        ),
        format!(
            "Obswiki vault `AGENTS.md` 正文如下:\n{}",
            context.agents_body.trim()
        ),
        format!(
            "Obswiki vault `index.md` 链接索引如下:\n{}",
            context.index_body.trim()
        ),
    ];
    info!(
        prompt_count = prompts.len(),
        vault_path = %preflight.vault_path.display(),
        "built obswiki child-thread initialization prompts"
    );
    Ok(prompts)
}
