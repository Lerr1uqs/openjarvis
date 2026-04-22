//! Built-in `edit` tool implementation that replaces the first exact text match in a file.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path};

#[derive(Default)]
pub struct EditTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditToolArguments {
    /// Path to the text file that should be edited.
    path: String,
    /// The existing text that should be replaced exactly once.
    #[serde(alias = "old")]
    old_text: String,
    /// The replacement text.
    #[serde(alias = "new")]
    new_text: String,
}

impl EditTool {
    /// Create the built-in `edit` tool.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for EditTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<EditToolArguments>(
            "edit",
            "Edit a UTF-8 text file by replacing the first exact matched string and writing back the full file.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        self.call_with_context(super::ToolCallContext::default(), request)
            .await
    }

    async fn call_with_context(
        &self,
        context: super::ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let args: EditToolArguments = parse_tool_arguments(request, "edit")?;
        if args.old_text.is_empty() {
            bail!("edit tool requires non-empty `old_text`");
        }

        let match_count = match context.active_sandbox() {
            Some(sandbox) => {
                sandbox
                    .edit_workspace_text(Path::new(&args.path), &args.old_text, &args.new_text)
                    .with_context(|| format!("failed to edit sandbox file {}", args.path))?
                    .match_count
            }
            None => {
                let content = fs::read_to_string(&args.path)
                    .with_context(|| format!("failed to read file {}", args.path))?;
                let match_count = content.matches(&args.old_text).count();
                if match_count == 0 {
                    bail!("edit tool did not find target text in {}", args.path);
                }
                let updated = content.replacen(&args.old_text, &args.new_text, 1);
                fs::write(&args.path, &updated)
                    .with_context(|| format!("failed to write file {}", args.path))?;
                match_count
            }
        };

        Ok(ToolCallResult {
            content: "updated 1 occurrence".to_string(),
            metadata: json!({
                "path": args.path,
                "match_count": match_count,
                "replaced_count": 1,
            }),
            is_error: false,
        })
    }
}
