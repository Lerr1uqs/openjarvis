//! `load_skill` tool that exposes progressive local skill loading to the agent loop.

use super::registry::SkillRegistry;
use crate::agent::tool::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub struct LoadSkillTool {
    registry: Arc<SkillRegistry>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoadSkillToolArguments {
    /// Exact skill name to load.
    name: String,
}

impl LoadSkillTool {
    /// Create the `load_skill` tool backed by one local skill registry.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::{LoadSkillTool, SkillRegistry};
    /// use std::sync::Arc;
    ///
    /// let _tool = LoadSkillTool::new(Arc::new(SkillRegistry::new()));
    /// ```
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolHandler for LoadSkillTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<LoadSkillToolArguments>(
            "load_skill",
            "Load one enabled local skill by exact name and return its full instructions plus referenced files.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: LoadSkillToolArguments = parse_tool_arguments(request, "load_skill")?;
        let skill_name = args.name.trim();
        if skill_name.is_empty() {
            bail!("load_skill requires a non-empty `name`");
        }

        // TODO: gate remote skill fetch, updates, and approval workflow before loading untrusted
        // external skills. Workspace-local `.openjarvis/skills` is the only supported source in
        // this V1.
        let loaded_skill = self.registry.load(skill_name).await?;
        let referenced_paths = loaded_skill
            .referenced_files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect::<Vec<_>>();

        Ok(ToolCallResult {
            content: loaded_skill.to_prompt(),
            metadata: json!({
                "name": loaded_skill.manifest.name,
                "description": loaded_skill.manifest.description,
                "skill_file": loaded_skill.manifest.skill_file,
                "referenced_files": referenced_paths,
                "referenced_file_count": loaded_skill.referenced_files.len(),
            }),
            is_error: false,
        })
    }
}
