use super::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use tokio::{
    process::Command,
    time::{Duration, timeout},
};

const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;
#[cfg(windows)]
const WINDOWS_UTF8_POWERSHELL_PREFIX: &str = "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false); chcp.com 65001 > $null;";

#[derive(Default)]
pub struct ShellTool;

#[derive(Debug, Deserialize)]
struct ShellToolArguments {
    command: String,
    workdir: Option<String>,
    timeout_ms: Option<u64>,
}

impl ShellTool {
    pub fn new() -> Self {
        // 作用: 创建内置 shell 工具实例。
        // 参数: 无，shell 工具用于执行一条系统命令。
        Self
    }
}

#[async_trait]
impl ToolHandler for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "shell".to_string(),
            description: "Run a shell command in the local workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute."
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Optional working directory for the command."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Optional timeout in milliseconds."
                    }
                },
                "required": ["command"],
                "additionalProperties": false,
            }),
        }
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        // 作用: 执行一条 shell 命令，并返回 stdout/stderr 与退出状态。
        // 参数: request.arguments 需要包含 command，可选 workdir 和 timeout_ms。
        let args: ShellToolArguments =
            serde_json::from_value(request.arguments).context("invalid shell tool arguments")?;
        let mut command = build_shell_command(&args.command);
        if let Some(workdir) = args.workdir.as_deref() {
            command.current_dir(workdir);
        }
        command.stdin(Stdio::null());

        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_SHELL_TIMEOUT_MS);
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
                "workdir": args.workdir,
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
    // 作用: 根据当前平台构造一条执行 shell 命令的进程。
    // 参数: command 为需要执行的原始命令字符串。
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
    // 作用: 在 Windows PowerShell 下兼容少量常见的 Unix 风格命令，避免模型直接调用时报错。
    // 参数: command 为模型生成的原始 shell 命令字符串。
    match command.trim() {
        "env" | "printenv" => {
            "Get-ChildItem Env: | Sort-Object Name | ForEach-Object { \"{0}={1}\" -f $_.Name, $_.Value }"
                .to_string()
        }
        _ => command.to_string(),
    }
}
