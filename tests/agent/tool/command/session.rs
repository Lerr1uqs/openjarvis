use super::command_session_fixture;
use openjarvis::agent::{
    CommandExecutionRequest, CommandSessionManager, CommandTaskStatus, CommandWriteRequest,
};

#[tokio::test]
async fn command_session_manager_tracks_background_completion_and_snapshots() {
    // 测试场景: 后台命令先返回 session_id，随后轮询拿到终态输出，并且导出快照从 Doing 迁移到 Done。
    let manager = CommandSessionManager::new();
    let request = if cfg!(windows) {
        CommandExecutionRequest {
            cmd: "Start-Sleep -Milliseconds 120; Write-Output 'done-background'".to_string(),
            shell: Some("powershell".to_string()),
            yield_time_ms: 10,
            ..CommandExecutionRequest::new("")
        }
    } else {
        CommandExecutionRequest {
            cmd: "sleep 0.12; printf 'done-background'".to_string(),
            yield_time_ms: 10,
            ..CommandExecutionRequest::new("")
        }
    };

    let started = manager
        .exec_command("thread-background", request)
        .await
        .expect("background command should start");
    let session_id = started
        .session_id
        .clone()
        .expect("background command should expose a session id");
    assert!(started.running);

    let unread_tasks = manager.list_unread_tasks("thread-background").await;
    assert!(unread_tasks.is_empty());
    let snapshots = manager.export_task_snapshots().await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].session_id, session_id);
    assert_eq!(snapshots[0].status, CommandTaskStatus::Doing);
    assert!(!snapshots[0].has_unread_output);

    let finished = manager
        .write_stdin(
            "thread-background",
            CommandWriteRequest {
                session_id: session_id.clone(),
                chars: String::new(),
                yield_time_ms: 400,
                max_output_tokens: None,
            },
        )
        .await
        .expect("background command should finish on poll");

    assert!(!finished.running);
    assert_eq!(finished.exit_code, Some(0));
    assert!(finished.output.contains("done-background"));
    assert!(
        manager
            .list_unread_tasks("thread-background")
            .await
            .is_empty()
    );

    let snapshots = manager.export_task_snapshots().await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].session_id, session_id);
    assert_eq!(snapshots[0].status, CommandTaskStatus::Done);
    assert!(!snapshots[0].has_unread_output);
    assert_eq!(snapshots[0].exit_code, Some(0));
}

#[tokio::test]
async fn command_session_manager_enforces_thread_isolation() {
    // 测试场景: 其他线程不能续写不属于自己的 session，而且原线程中的后台任务继续存在。
    let manager = CommandSessionManager::new();
    let request = if cfg!(windows) {
        CommandExecutionRequest {
            cmd: "Start-Sleep -Milliseconds 200".to_string(),
            shell: Some("powershell".to_string()),
            yield_time_ms: 10,
            ..CommandExecutionRequest::new("")
        }
    } else {
        CommandExecutionRequest {
            cmd: "sleep 0.2".to_string(),
            yield_time_ms: 10,
            ..CommandExecutionRequest::new("")
        }
    };
    let started = manager
        .exec_command("thread-owner", request)
        .await
        .expect("background command should start");
    let session_id = started
        .session_id
        .expect("background command should expose session id");

    let error = manager
        .write_stdin(
            "thread-other",
            CommandWriteRequest {
                session_id: session_id.clone(),
                chars: String::new(),
                yield_time_ms: 10,
                max_output_tokens: None,
            },
        )
        .await
        .expect_err("foreign thread should not access the session");
    assert!(error.to_string().contains("does not belong to thread"));
    let snapshots = manager.export_task_snapshots().await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].status, CommandTaskStatus::Doing);
}

#[tokio::test]
async fn command_session_manager_drives_interactive_fixture() {
    // 测试场景: 多轮 stdin 续写可以驱动真实交互程序直到输出 OK。
    let Some(command) = command_session_fixture() else {
        return;
    };
    let manager = CommandSessionManager::new();
    let started = manager
        .exec_command(
            "thread-interactive",
            CommandExecutionRequest {
                cmd: command,
                tty: false,
                yield_time_ms: 100,
                ..CommandExecutionRequest::new("")
            },
        )
        .await
        .expect("fixture should start");
    let session_id = started
        .session_id
        .clone()
        .expect("fixture should remain interactive");
    let initial_output = if started.output.contains("FIRST?") {
        started.output
    } else {
        // 首屏 prompt 可能恰好错过第一次等待窗，这里再用一次空写轮询补拿增量输出。
        manager
            .write_stdin(
                "thread-interactive",
                CommandWriteRequest {
                    session_id: session_id.clone(),
                    chars: String::new(),
                    yield_time_ms: 100,
                    max_output_tokens: None,
                },
            )
            .await
            .expect("fixture should expose the initial prompt on the next poll")
            .output
    };
    assert!(initial_output.contains("FIRST?"));

    let after_first = manager
        .write_stdin(
            "thread-interactive",
            CommandWriteRequest {
                session_id: session_id.clone(),
                chars: "2\n".to_string(),
                yield_time_ms: 100,
                max_output_tokens: None,
            },
        )
        .await
        .expect("fixture should accept the first number");
    assert!(after_first.output.contains("FIRST=2"));
    assert!(after_first.output.contains("SECOND?"));

    let after_second = manager
        .write_stdin(
            "thread-interactive",
            CommandWriteRequest {
                session_id: session_id.clone(),
                chars: "3\n".to_string(),
                yield_time_ms: 100,
                max_output_tokens: None,
            },
        )
        .await
        .expect("fixture should accept the second number");
    assert!(after_second.output.contains("SECOND=3"));
    assert!(after_second.running);

    let finished = manager
        .write_stdin(
            "thread-interactive",
            CommandWriteRequest {
                session_id,
                chars: "5\n".to_string(),
                yield_time_ms: 200,
                max_output_tokens: None,
            },
        )
        .await
        .expect("fixture should accept the computed sum");
    assert!(!finished.running);
    assert_eq!(finished.exit_code, Some(0));
    assert!(finished.output.contains("OK"));
}
