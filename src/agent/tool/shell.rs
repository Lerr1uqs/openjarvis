//! Built-in `bash` tool implementation for one-shot shell command execution.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, parse_tool_arguments,
    tool_definition_from_args,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use tokio::{
    process::Command,
    time::{Duration, timeout},
};

const DEFAULT_BASH_TIMEOUT_MS: u64 = 30_000;
#[cfg(windows)]
const WINDOWS_UTF8_POWERSHELL_PREFIX: &str = "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false); chcp.com 65001 > $null;";

#[derive(Default)]
pub struct ShellTool;

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
        Self
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
        let mut command = build_shell_command(&args.command);
        command.stdin(Stdio::null());

        let timeout_ms = args.timeout.unwrap_or(DEFAULT_BASH_TIMEOUT_MS);
        let output = match timeout(Duration::from_millis(timeout_ms), command.output()).await {
            Ok(result) => result.context("failed to execute shell command")?,
            Err(_) => {
                return Ok(ToolCallResult {
                    content: format!("command timed out after {} ms", timeout_ms),
                    metadata: json!({
                        "command": args.command,
                        "timeout_ms": timeout_ms,
                    }),
                    is_error: true,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status_code = output.status.code();
        let content = if output.status.success() {
            stdout.clone()
        } else if stderr.trim().is_empty() {
            stdout.clone()
        } else {
            stderr.clone()
        };

        Ok(ToolCallResult {
            content,
            metadata: json!({
                "command": args.command,
                "timeout_ms": timeout_ms,
                "status_code": status_code,
                "stdout": stdout,
                "stderr": stderr,
            }),
            is_error: !output.status.success(),
        })
    }
}

fn build_shell_command(command: &str) -> Command {
    // Build the platform-specific process wrapper used to execute one command string.
    #[cfg(windows)]
    {
        let command = normalize_windows_command(command);
        let mut process = Command::new("powershell");
        process
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(format!("{WINDOWS_UTF8_POWERSHELL_PREFIX} {command}"));
        process
    }

    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.arg("-lc").arg(command);
        process
    }
}

#[cfg(windows)]
fn normalize_windows_command(command: &str) -> String {
    // Translate a few common Unix-style commands into PowerShell-friendly equivalents.
    match command.trim() {
        "env" | "printenv" => {
            "Get-ChildItem Env: | Sort-Object Name | ForEach-Object { \"{0}={1}\" -f $_.Name, $_.Value }"
                .to_string()
        }
        _ => command.to_string(),
    }
}
