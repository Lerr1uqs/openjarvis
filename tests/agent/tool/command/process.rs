use super::super::{build_thread, call_tool};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Read, path::Path, process::Command};
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

const EXEC_HELPER_SECCOMP_PROBE_CASES: &[&str] = &[
    "open_tree_missing",
    "move_mount_invalid",
    "mount_setattr_invalid",
    "clone3_invalid",
];

fn exec_helper_seccomp_probe_source() -> &'static str {
    r#"use std::{env, ffi::CString, io, process};

unsafe extern "C" {
    fn syscall(number: isize, ...) -> isize;
}

const EPERM: i32 = 1;
const AT_FDCWD: isize = -100;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64"))]
const SYS_OPEN_TREE: isize = 428;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64"))]
const SYS_MOVE_MOUNT: isize = 429;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64"))]
const SYS_CLONE3: isize = 435;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64"))]
const SYS_MOUNT_SETATTR: isize = 442;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
compile_error!("seccomp probe only supports x86_64, aarch64, and riscv64");

fn syscall_errno(result: isize) -> i32 {
    if result == -1 {
        io::Error::last_os_error().raw_os_error().unwrap_or(-1)
    } else {
        0
    }
}

fn probe_open_tree_missing() -> i32 {
    let missing = CString::new("/openjarvis-seccomp-probe-missing").expect("missing probe path");
    syscall_errno(unsafe { syscall(SYS_OPEN_TREE, AT_FDCWD, missing.as_ptr(), 0usize) })
}

fn probe_move_mount_invalid() -> i32 {
    syscall_errno(unsafe {
        syscall(
            SYS_MOVE_MOUNT,
            -1isize,
            std::ptr::null::<u8>(),
            -1isize,
            std::ptr::null::<u8>(),
            0usize,
        )
    })
}

fn probe_mount_setattr_invalid() -> i32 {
    syscall_errno(unsafe {
        syscall(
            SYS_MOUNT_SETATTR,
            -1isize,
            std::ptr::null::<u8>(),
            0usize,
            std::ptr::null::<u8>(),
            0usize,
        )
    })
}

fn probe_clone3_invalid() -> i32 {
    syscall_errno(unsafe { syscall(SYS_CLONE3, std::ptr::null::<u8>(), 0usize) })
}

fn main() {
    let mut denied = true;
    for case in env::args().skip(1) {
        let errno = match case.as_str() {
            "open_tree_missing" => probe_open_tree_missing(),
            "move_mount_invalid" => probe_move_mount_invalid(),
            "mount_setattr_invalid" => probe_mount_setattr_invalid(),
            "clone3_invalid" => probe_clone3_invalid(),
            other => {
                eprintln!("unknown seccomp probe case: {other}");
                process::exit(2);
            }
        };
        println!("{case}={errno}");
        if errno != EPERM {
            denied = false;
        }
    }
    process::exit(if denied { 0 } else { 1 });
}
"#
}

fn compile_exec_helper_seccomp_probe(workspace_root: &Path) -> std::path::PathBuf {
    let source_path = workspace_root.join("helper-seccomp-probe.rs");
    let binary_path = workspace_root.join("helper-seccomp-probe");
    fs::write(&source_path, exec_helper_seccomp_probe_source())
        .expect("helper seccomp probe source should be written");
    let output = Command::new("rustc")
        .arg("--edition=2021")
        .arg(&source_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("helper seccomp probe should compile");
    if !output.status.success() {
        panic!(
            "failed to compile helper seccomp probe: stdout=`{}` stderr=`{}`",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    binary_path
}

fn parse_exec_helper_probe_errno_map(stdout: &[u8]) -> BTreeMap<String, i32> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let (case_name, errno_text) = line.split_once('=')?;
            Some((case_name.to_string(), errno_text.parse().ok()?))
        })
        .collect()
}

fn supported_exec_helper_probe_cases(probe_binary: &Path) -> Vec<&'static str> {
    let output = Command::new(probe_binary)
        .args(EXEC_HELPER_SECCOMP_PROBE_CASES)
        .output()
        .expect("helper probe should execute on host");
    let errno_map = parse_exec_helper_probe_errno_map(&output.stdout);

    EXEC_HELPER_SECCOMP_PROBE_CASES
        .iter()
        .copied()
        .filter(|case_name| {
            errno_map
                .get(*case_name)
                .is_some_and(|errno| *errno != libc::EPERM && *errno != libc::ENOSYS)
        })
        .collect()
}

#[test]
fn internal_sandbox_exec_helper_installs_final_seccomp_on_pipe_path() {
    // 测试场景: 最终 command helper 直连 pipe 路径时，应只在最终 exec 前安装 seccomp，并拒绝逃逸导向 syscall。
    if !host_supports_internal_sandbox_exec_helper() {
        return;
    }

    let workspace_root =
        std::env::temp_dir().join(format!("openjarvis-command-helper-pipe-{}", Uuid::new_v4()));
    fs::create_dir_all(&workspace_root).expect("pipe helper workspace should be created");
    let probe_binary = compile_exec_helper_seccomp_probe(&workspace_root);
    let supported_cases = supported_exec_helper_probe_cases(&probe_binary);
    if supported_cases.is_empty() {
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .args(helper_command_args(
            &workspace_root,
            &format!("{} {}", probe_binary.display(), supported_cases.join(" ")),
        ))
        .output()
        .expect("helper pipe command should execute");

    assert!(
        output.status.success(),
        "final helper pipe path should allow normal command start and enforce seccomp in-child"
    );
    for case_name in &supported_cases {
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("{case_name}={}", libc::EPERM)),
            "final helper pipe path should deny `{case_name}` with EPERM"
        );
    }
    let _ = fs::remove_dir_all(&workspace_root);
}

#[test]
fn internal_sandbox_exec_helper_installs_final_seccomp_on_pty_path() {
    // 测试场景: 最终 command helper 通过 PTY 拉起时，也应安装同一 seccomp，避免 PTY 分支绕过最终收口。
    if !host_supports_internal_sandbox_exec_helper() {
        return;
    }

    let workspace_root =
        std::env::temp_dir().join(format!("openjarvis-command-helper-pty-{}", Uuid::new_v4()));
    fs::create_dir_all(&workspace_root).expect("pty helper workspace should be created");
    let probe_binary = compile_exec_helper_seccomp_probe(&workspace_root);
    let supported_cases = supported_exec_helper_probe_cases(&probe_binary);
    if supported_cases.is_empty() {
        return;
    }

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
        &format!("{} {}", probe_binary.display(), supported_cases.join(" ")),
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

    assert_eq!(
        exit_code, 0,
        "final helper PTY path should return probe success"
    );
    for case_name in &supported_cases {
        assert!(
            output.contains(&format!("{case_name}={}", libc::EPERM)),
            "final helper PTY path should deny `{case_name}` with EPERM"
        );
    }
    let _ = fs::remove_dir_all(&workspace_root);
}
