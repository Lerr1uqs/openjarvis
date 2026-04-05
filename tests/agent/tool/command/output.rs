use super::super::{build_thread, call_tool};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use serde_json::json;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn command_tools_render_stable_plain_text_summary_and_truncation() {
    // 测试场景: exec_command 返回固定顺序的纯文本摘要，并在 max_output_tokens 下暴露裁剪前规模。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-command-output-summary");
    let output_payload = "0123456789abcdef".repeat(20);
    let command = if cfg!(windows) {
        format!("Write-Output '{}'", output_payload)
    } else {
        format!("printf '{}'", output_payload)
    };

    let result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments: json!({
                "cmd": command,
                "yield_time_ms": 500,
                "max_output_tokens": 2
            }),
        },
    )
    .await
    .expect("exec_command should succeed");

    assert!(result.content.starts_with("Command: "));
    assert!(result.content.contains("\nChunk ID: "));
    assert!(result.content.contains("\nWall time: "));
    assert!(result.content.contains("Process exited with code 0"));
    assert!(result.content.contains("\nOriginal token count: "));
    assert!(result.content.contains("\nOutput:\n"));
    let original_token_count = result.metadata["original_token_count"]
        .as_u64()
        .expect("original token count should be present");
    let output = result.metadata["output"]
        .as_str()
        .expect("truncated output should be present");
    assert!(original_token_count > 2);
    assert!(output.len() < output_payload.len());
}

#[tokio::test]
async fn command_tools_report_wall_time_for_the_current_call_instead_of_session_lifetime() {
    // 测试场景: wall_time_seconds 表示本次工具调用的墙钟耗时，而不是 session 启动以来的累计时间。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-command-output-wall-time");

    let started = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments: if cfg!(windows) {
                json!({
                    "cmd": "Start-Sleep -Milliseconds 150; Write-Output 'wall-time-finished'",
                    "shell": "powershell",
                    "yield_time_ms": 20
                })
            } else {
                json!({
                    "cmd": "sleep 0.15; printf 'wall-time-finished'",
                    "yield_time_ms": 20
                })
            },
        },
    )
    .await
    .expect("exec_command should start a background session");
    let session_id = started.metadata["session_id"]
        .as_str()
        .expect("background command should expose session id")
        .to_string();
    let started_wall_time = started.metadata["wall_time_seconds"]
        .as_f64()
        .expect("wall_time_seconds should be present for exec_command");
    assert!(started_wall_time < 0.20);

    sleep(Duration::from_millis(260)).await;

    let finished = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 20
            }),
        },
    )
    .await
    .expect("write_stdin should drain the finished command");
    let finished_wall_time = finished.metadata["wall_time_seconds"]
        .as_f64()
        .expect("wall_time_seconds should be present for write_stdin");
    assert!(finished_wall_time < 0.20);
    assert_eq!(finished.metadata["exit_code"], 0);
}

#[tokio::test]
async fn command_tools_render_drained_placeholder_when_process_exits_without_output() {
    // 测试场景: 进程已退出且没有任何未读输出时，摘要应显式说明缓冲区已经读完。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-command-output-drained");

    let result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments: if cfg!(windows) {
                json!({
                    "cmd": "exit 0",
                    "shell": "powershell",
                    "yield_time_ms": 200
                })
            } else {
                json!({
                    "cmd": "true",
                    "yield_time_ms": 200
                })
            },
        },
    )
    .await
    .expect("exec_command should succeed for an empty-output command");

    assert!(result.content.contains("Process exited with code 0"));
    assert!(
        result
            .content
            .contains("Output: NULL (当前程序已结束，缓冲区读取完毕)")
    );
    assert_eq!(result.metadata["output"], "");
    assert_eq!(result.metadata["original_token_count"], 0);
}
