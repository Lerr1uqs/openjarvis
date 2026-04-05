use super::super::{build_thread, call_tool};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use serde_json::json;
use std::{fs, process::Command};
use uuid::Uuid;

#[tokio::test]
async fn exec_command_honors_workdir_and_optional_shell() {
    // 测试场景: exec_command 能把命令放到指定工作目录，并兼容显式 shell 选择。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread-command-workdir-shell");
    let workdir =
        std::env::temp_dir().join(format!("openjarvis-command-workdir-{}", Uuid::new_v4()));
    fs::create_dir_all(&workdir).expect("workdir should be created");

    let mut arguments = if cfg!(windows) {
        json!({
            "cmd": "Get-Location | Select-Object -ExpandProperty Path",
            "shell": "powershell",
            "workdir": workdir,
            "yield_time_ms": 500
        })
    } else {
        json!({
            "cmd": "pwd",
            "workdir": workdir,
            "yield_time_ms": 500
        })
    };

    if !cfg!(windows) && Command::new("bash").arg("--version").output().is_ok() {
        arguments["shell"] = json!("bash");
    }

    let result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "exec_command".to_string(),
            arguments,
        },
    )
    .await
    .expect("exec_command should honor workdir and shell");

    let workdir_text = workdir.to_string_lossy();
    assert!(result.content.contains(workdir_text.as_ref()));
    let _ = fs::remove_dir_all(&workdir);
}
