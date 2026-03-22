//! Built-in `read` tool implementation for UTF-8 files and optional line slicing.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::fs;

#[derive(Default)]
pub struct ReadTool;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadToolArguments {
    /// Path to the UTF-8 text file to read.
    path: String,
    /// Optional 1-based inclusive start line.
    start_line: Option<usize>,
    /// Optional 1-based inclusive end line.
    end_line: Option<usize>,
}

impl ReadTool {
    /// Create the built-in `read` tool.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for ReadTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ReadToolArguments>(
            "read",
            "Read a UTF-8 text file from disk, optionally by 1-based inclusive line range.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ReadToolArguments = parse_tool_arguments(request, "read")?;
        let content = fs::read_to_string(&args.path)
            .with_context(|| format!("failed to read file {}", args.path))?;
        let selected = select_line_range(&content, args.start_line, args.end_line)?;

        Ok(ToolCallResult {
            content: selected.clone(),
            metadata: json!({
                "path": args.path,
                "start_line": args.start_line,
                "end_line": args.end_line,
                "returned_line_count": count_lines(&selected),
            }),
            is_error: false,
        })
    }
}

fn select_line_range(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String> {
    if start_line.is_none() && end_line.is_none() {
        return Ok(content.to_string());
    }

    let start_line = start_line.unwrap_or(1);
    let end_line = end_line.unwrap_or(usize::MAX);

    if start_line == 0 || end_line == 0 {
        bail!("read tool line numbers are 1-based and must be greater than 0");
    }
    if start_line > end_line {
        bail!("read tool requires start_line <= end_line");
    }
    if content.is_empty() {
        if start_line > 1 {
            bail!(
                "read tool start_line {} is out of range for 0 total lines",
                start_line
            );
        }
        return Ok(String::new());
    }

    let lines = content.split_inclusive('\n').collect::<Vec<_>>();
    if start_line > lines.len() {
        bail!(
            "read tool start_line {} is out of range for {} total lines",
            start_line,
            lines.len()
        );
    }

    let end_line = end_line.min(lines.len());
    Ok(lines[(start_line - 1)..end_line].concat())
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.split_inclusive('\n').count()
    }
}
