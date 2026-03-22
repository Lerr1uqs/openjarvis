use openjarvis::agent::{ShellTool, ToolCallRequest, ToolHandler};
use serde_json::json;

#[tokio::test]
async fn shell_tool_executes_command() {
    let tool = ShellTool::new();
    let command = if cfg!(windows) {
        "Write-Output 'hello-shell'"
    } else {
        "printf 'hello-shell'"
    };

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": command,
                "timeout": 5_000,
            }),
        })
        .await
        .expect("shell tool should run");

    assert!(result.content.contains("hello-shell"));
    assert!(!result.is_error);
}

#[tokio::test]
async fn shell_tool_preserves_utf8_output() {
    let tool = ShellTool::new();
    let command = if cfg!(windows) {
        "Write-Output '中文输出'"
    } else {
        "printf '中文输出'"
    };

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": command,
                "timeout": 5_000,
            }),
        })
        .await
        .expect("shell tool should run");

    assert!(result.content.contains("中文输出"));
    assert!(!result.is_error);
}

#[tokio::test]
async fn shell_tool_supports_env_command() {
    let tool = ShellTool::new();

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": "env",
                "timeout": 5_000,
            }),
        })
        .await
        .expect("shell tool should run");

    assert!(!result.is_error);
    assert!(result.content.contains('='));
}

#[tokio::test]
async fn shell_tool_returns_stderr_for_failed_commands() {
    let tool = ShellTool::new();
    let command = if cfg!(windows) {
        "Write-Error 'boom'; exit 7"
    } else {
        "printf 'boom' >&2; exit 7"
    };

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": command,
                "timeout": 5_000,
            }),
        })
        .await
        .expect("shell tool should return a failed result");

    assert!(result.is_error);
    assert!(result.content.contains("boom"));
    assert_eq!(result.metadata["status_code"], 7);
}

#[tokio::test]
async fn shell_tool_times_out() {
    let tool = ShellTool::new();
    let command = if cfg!(windows) {
        "Start-Sleep -Milliseconds 200; Write-Output 'late'"
    } else {
        "sleep 1; printf 'late'"
    };

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": command,
                "timeout": 10,
            }),
        })
        .await
        .expect("shell tool should return a timeout result");

    assert!(result.is_error);
    assert!(result.content.contains("timed out"));
    assert_eq!(result.metadata["timeout_ms"], 10);
}

#[tokio::test]
async fn shell_tool_supports_timeout_ms_alias() {
    let tool = ShellTool::new();
    let command = if cfg!(windows) {
        "Write-Output 'timeout-alias'"
    } else {
        "printf 'timeout-alias'"
    };

    let result = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": command,
                "timeout_ms": 5_000,
            }),
        })
        .await
        .expect("shell tool should accept timeout_ms alias");

    assert!(!result.is_error);
    assert!(result.content.contains("timeout-alias"));
}

#[tokio::test]
async fn shell_tool_rejects_unknown_arguments() {
    let tool = ShellTool::new();

    let error = tool
        .call(ToolCallRequest {
            name: "bash".to_string(),
            arguments: json!({
                "command": "env",
                "workdir": ".",
            }),
        })
        .await
        .expect_err("shell tool should reject unknown arguments");

    assert!(format!("{error:#}").contains("unknown field"));
}
