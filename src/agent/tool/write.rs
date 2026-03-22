//! Built-in `write` tool implementation for full-file UTF-8 overwrites.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path};

#[derive(Default)]
pub struct WriteTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteToolArguments {
    /// Path to the file that should be written.
    path: String,
    /// Full UTF-8 text content to write into the file.
    content: String,
}

impl WriteTool {
    /// Create the built-in `write` tool.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for WriteTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<WriteToolArguments>(
            "write",
            "Write full UTF-8 text content to a file, overwriting any existing content.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: WriteToolArguments = parse_tool_arguments(request, "write")?;
        if let Some(parent) = Path::new(&args.path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directories for {}", args.path)
                })?;
            }
        }

        fs::write(&args.path, &args.content)
            .with_context(|| format!("failed to write file {}", args.path))?;

        Ok(ToolCallResult {
            content: format!("wrote {} bytes", args.content.len()),
            metadata: json!({
                "path": args.path,
                "bytes_written": args.content.len(),
            }),
            is_error: false,
        })
    }
}
