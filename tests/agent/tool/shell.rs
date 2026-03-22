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
            name: "shell".to_string(),
            arguments: json!({
                "command": command,
                "timeout_ms": 5_000,
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
            name: "shell".to_string(),
            arguments: json!({
                "command": command,
                "timeout_ms": 5_000,
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
            name: "shell".to_string(),
            arguments: json!({
                "command": "env",
                "timeout_ms": 5_000,
            }),
        })
        .await
        .expect("shell tool should run");

    assert!(!result.is_error);
    assert!(result.content.contains('='));
}
