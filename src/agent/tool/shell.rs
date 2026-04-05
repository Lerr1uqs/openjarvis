//! Built-in `bash` tool implementation for one-shot shell command execution.

use super::command::{CommandSessionManager, run_legacy_shell_command};
use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const DEFAULT_BASH_TIMEOUT_MS: u64 = 30_000;

pub struct ShellTool {
    sessions: Arc<CommandSessionManager>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellToolArguments {
    /// Shell command to execute.
    command: String,
    /// Optional timeout in milliseconds.
    #[serde(alias = "timeout_ms")]
    timeout: Option<u64>,
}

impl ShellTool {
    /// Create the built-in shell tool that backs the exposed `bash` capability.
    pub fn new() -> Self {
        Self::with_sessions(Arc::new(CommandSessionManager::new()))
    }

    pub fn with_sessions(sessions: Arc<CommandSessionManager>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl ToolHandler for ShellTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ShellToolArguments>(
            "bash",
            "Run a local shell command. `timeout` is measured in milliseconds.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ShellToolArguments = parse_tool_arguments(request, "bash")?;
        let timeout_ms = args.timeout.unwrap_or(DEFAULT_BASH_TIMEOUT_MS);
        let _ = &self.sessions;
        let output = run_legacy_shell_command(&args.command, timeout_ms).await?;
        if output.timed_out {
            return Ok(ToolCallResult {
                content: format!("command timed out after {} ms", timeout_ms),
                metadata: json!({
                    "command": args.command,
                    "timeout_ms": timeout_ms,
                }),
                is_error: true,
            });
        }

        let content = if output.status_code == Some(0) {
            output.stdout.clone()
        } else if output.stderr.trim().is_empty() {
            output.stdout.clone()
        } else {
            output.stderr.clone()
        };

        Ok(ToolCallResult {
            content,
            metadata: json!({
                "command": args.command,
                "timeout_ms": timeout_ms,
                "status_code": output.status_code,
                "stdout": output.stdout,
                "stderr": output.stderr,
            }),
            is_error: output.status_code != Some(0),
        })
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}
