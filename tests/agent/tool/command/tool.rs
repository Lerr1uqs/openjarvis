use super::super::{build_thread, call_tool};
use super::command_session_fixture;
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use serde_json::json;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn exec_command_and_list_unread_command_tasks_are_registered() {
    // 测试场景: builtin tool 注册后，新命令会话工具和兼容 bash 会同时可见。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let mut names = registry
        .list()
        .await
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "bash",
            "edit",
            "exec_command",
            "list_unread_command_tasks",
            "read",
            "write",
            "write_stdin",
        ]
    );
}

#[tokio::test]
async fn list_unread_command_tasks_only_returns_sessions_with_unread_output() {
    // 测试场景: list_unread_command_tasks 只列出当前线程里还有未读输出的 session。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-list-unread-command-tasks");

    let started = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments: if cfg!(windows) {
                json!({
                    "cmd": "Start-Sleep -Milliseconds 50; Write-Output 'list-finished'",
                    "shell": "powershell",
                    "yield_time_ms": 10
                })
            } else {
                json!({
                    "cmd": "sleep 0.05; printf 'list-finished'",
                    "yield_time_ms": 10
                })
            },
        },
    )
    .await
    .expect("background command should start");
    let session_id = started.metadata["session_id"]
        .as_str()
        .expect("background session id should be present")
        .to_string();

    let empty_before_output = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "list_unread_command_tasks".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("list_unread_command_tasks should succeed before new output arrives");
    assert_eq!(
        empty_before_output.content,
        "No unread command task output."
    );
    assert_eq!(empty_before_output.metadata["tasks"], json!([]));
    assert!(
        empty_before_output.metadata["wall_time_seconds"]
            .as_f64()
            .expect("list_unread_command_tasks should report wall_time_seconds")
            >= 0.0
    );

    sleep(Duration::from_millis(160)).await;

    let unread = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "list_unread_command_tasks".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("list_unread_command_tasks should surface unread output after the command emits");
    assert!(unread.content.contains(&session_id));
    assert!(unread.content.contains("exit_code=0"));
    assert_eq!(
        unread.metadata["tasks"]
            .as_array()
            .expect("list_unread_command_tasks should return a task array")
            .len(),
        1
    );
    assert!(
        unread.metadata["wall_time_seconds"]
            .as_f64()
            .expect("list_unread_command_tasks should report wall_time_seconds")
            >= 0.0
    );

    let finished = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 300
            }),
        },
    )
    .await
    .expect("write_stdin poll should observe natural exit");
    assert!(finished.content.contains("Process exited with code 0"));

    let empty = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "list_unread_command_tasks".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("list_unread_command_tasks should succeed after unread output is drained");
    assert_eq!(empty.content, "No unread command task output.");
}

#[tokio::test]
async fn write_stdin_returns_error_for_unknown_session() {
    // 测试场景: write_stdin 对未知 session_id 显式报错。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-unknown-command-session");

    let error = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": "missing-command-session",
                "chars": ""
            }),
        },
    )
    .await
    .expect_err("unknown session should fail");
    assert!(error.to_string().contains("unknown command session"));
}

#[tokio::test]
async fn tty_fixture_round_trip_can_finish_with_ok() {
    // 测试场景: exec_command + write_stdin 可以通过 tty 路径驱动交互 fixture 到自然退出。
    let Some(command) = command_session_fixture() else {
        return;
    };
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-command-tty-fixture");

    let started = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments: json!({
                "cmd": command,
                "tty": true,
                "yield_time_ms": 100
            }),
        },
    )
    .await
    .expect("tty fixture should start");
    let session_id = started.metadata["session_id"]
        .as_str()
        .expect("tty session should expose session id")
        .to_string();
    assert!(started.content.contains("Process running with session ID"));

    let after_first = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "4\n",
                "yield_time_ms": 100
            }),
        },
    )
    .await
    .expect("first tty write should succeed");
    let session_id = after_first.metadata["session_id"]
        .as_str()
        .expect("session should keep running after the first tty write")
        .to_string();

    let after_second = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "7\n",
                "yield_time_ms": 100
            }),
        },
    )
    .await
    .expect("second tty write should succeed");
    let session_id = after_second.metadata["session_id"]
        .as_str()
        .expect("session should keep running before the final answer")
        .to_string();

    let polled = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "",
                "yield_time_ms": 50
            }),
        },
    )
    .await
    .expect("empty write should poll tty session");
    let session_id = polled.metadata["session_id"]
        .as_str()
        .expect("poll should keep the tty session alive")
        .to_string();

    let finished = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "write_stdin".to_string(),
            arguments: json!({
                "session_id": session_id,
                "chars": "11\n",
                "yield_time_ms": 150
            }),
        },
    )
    .await
    .expect("final tty write should finish the fixture");
    assert!(finished.content.contains("Process exited with code 0"));
    assert!(finished.content.contains("OK"));
}
