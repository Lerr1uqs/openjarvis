use super::super::{build_thread, call_tool};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use std::{fs, io::Read, process::Command};
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

fn readonly_profile_json() -> &'static str {
    r#"{"name":"readonly","landlock_profile":"command-readonly","seccomp_profile":"command-readonly-v1","compatibility":{"require_landlock":true,"min_landlock_abi":1,"require_seccomp":true,"strict":true}}"#
}

fn helper_command_args(workspace_root: &std::path::Path, command: &str) -> Vec<String> {
    vec![
        "internal-sandbox".to_string(),
        "exec".to_string(),
        "--workspace-root".to_string(),
        workspace_root.display().to_string(),
        "--profile-json".to_string(),
        readonly_profile_json().to_string(),
        "--program".to_string(),
        "sh".to_string(),
        "--arg".to_string(),
        "-lc".to_string(),
        "--arg".to_string(),
        command.to_string(),
    ]
}

fn host_supports_internal_sandbox_exec_helper() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let workspace_root = std::env::temp_dir().join(format!(
        "openjarvis-command-helper-probe-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&workspace_root).expect("helper probe workspace should be created");
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .args(helper_command_args(&workspace_root, "true"))
        .output()
        .expect("helper probe should execute");
    let _ = fs::remove_dir_all(&workspace_root);
    output.status.success()
}

#[test]
fn internal_sandbox_exec_helper_blocks_workspace_writes_on_pipe_path() {
    // 测试场景: child helper 直连 pipe 路径时，应在真正 exec 前安装 readonly profile 并拒绝 workspace 写入。
    if !host_supports_internal_sandbox_exec_helper() {
        return;
    }

    let workspace_root =
        std::env::temp_dir().join(format!("openjarvis-command-helper-pipe-{}", Uuid::new_v4()));
    fs::create_dir_all(&workspace_root).expect("pipe helper workspace should be created");
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .args(helper_command_args(
            &workspace_root,
            "printf 'blocked' > helper-pipe.txt",
        ))
        .output()
        .expect("helper pipe command should execute");

    assert!(
        !output.status.success(),
        "readonly helper pipe profile should reject workspace writes"
    );
    assert!(
        !workspace_root.join("helper-pipe.txt").exists(),
        "readonly helper pipe profile should not leave written files behind"
    );
    let _ = fs::remove_dir_all(&workspace_root);
}

#[test]
fn internal_sandbox_exec_helper_blocks_workspace_writes_on_pty_path() {
    // 测试场景: child helper 通过 PTY 拉起时，也应安装同一 readonly profile，避免 PTY 分支绕过收口。
    if !host_supports_internal_sandbox_exec_helper() {
        return;
    }

    let workspace_root =
        std::env::temp_dir().join(format!("openjarvis-command-helper-pty-{}", Uuid::new_v4()));
    fs::create_dir_all(&workspace_root).expect("pty helper workspace should be created");

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("PTY pair should open");
    let mut builder = CommandBuilder::new(env!("CARGO_BIN_EXE_openjarvis"));
    builder.args(helper_command_args(
        &workspace_root,
        "printf 'blocked' > helper-pty.txt",
    ));
    let mut child = pair
        .slave
        .spawn_command(builder)
        .expect("PTY helper command should spawn");
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("PTY helper reader should clone");
    let mut output = String::new();
    let _ = reader.read_to_string(&mut output);
    let exit_code = child.wait().expect("PTY helper should exit").exit_code();

    assert_ne!(
        exit_code, 0,
        "readonly helper PTY profile should reject workspace writes"
    );
    assert!(
        !workspace_root.join("helper-pty.txt").exists(),
        "readonly helper PTY profile should not leave written files behind"
    );
    let _ = fs::remove_dir_all(&workspace_root);
}
