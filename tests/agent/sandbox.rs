use openjarvis::{
    agent::{
        AgentWorker, CommandExecutionRequest, CommandExecutionResult, CommandTaskSnapshot,
        CommandWriteRequest, SandboxBackendKind, SandboxCapabilityConfig, SandboxJsonRpcRequest,
        SandboxJsonRpcResponse, ToolCallRequest, ToolRegistry, build_sandbox,
    },
    llm::MockLLMProvider,
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Arc,
};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

mod kernel;

struct SandboxFixture {
    root: PathBuf,
}

impl SandboxFixture {
    fn new(prefix: &str) -> Self {
        let root = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("sandbox fixture root should be created");
        Self { root }
    }

    fn new_under_target(prefix: &str) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox-fixtures")
            .join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("sandbox fixture target root should be created");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write_capabilities_yaml(&self, yaml: &str) {
        let path = self.root.join("config/capabilities.yaml");
        fs::create_dir_all(path.parent().expect("capabilities parent should exist"))
            .expect("capabilities config directory should be created");
        fs::write(path, yaml).expect("capabilities yaml should be written");
    }

    fn install_fake_bwrap_wrapper(&self) -> PathBuf {
        // 通过 symlink 复用仓库内静态 wrapper，避免现写现执行脚本时偶发 ETXTBSY。
        let wrapper_path = self.root.join("fake-bwrap.sh");
        let helper_path = self.root.join("openjarvis-test-bin");
        install_helper_link(&repo_fake_bwrap_wrapper(), &wrapper_path);
        install_helper_link(Path::new(env!("CARGO_BIN_EXE_openjarvis")), &helper_path);
        wrapper_path
    }
}

impl Drop for SandboxFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct SandboxProxyFixture {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl SandboxProxyFixture {
    fn spawn(workspace_root: &Path) -> Self {
        let enforcement_plan_json = strict_proxy_enforcement_plan_json();
        let mut child = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
            .args([
                "internal-sandbox",
                "proxy",
                "--workspace-root",
                &workspace_root.display().to_string(),
                "--enforcement-plan-json",
                &enforcement_plan_json,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("sandbox proxy fixture should spawn");
        let stdin = BufWriter::new(
            child
                .stdin
                .take()
                .expect("sandbox proxy fixture stdin should exist"),
        );
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .expect("sandbox proxy fixture stdout should exist"),
        );
        let mut fixture = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let ping = fixture
            .call("rpc.ping", json!({}))
            .expect("sandbox proxy fixture ping should succeed");
        assert_eq!(ping["status"], json!("ok"));
        fixture
    }

    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let request = SandboxJsonRpcRequest::new(self.next_id, method, params);
        self.next_id += 1;
        let raw = serde_json::to_string(&request)?;
        self.stdin.write_all(raw.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        let mut line = String::new();
        let bytes = self.stdout.read_line(&mut line)?;
        if bytes == 0 {
            anyhow::bail!("sandbox proxy fixture closed before replying to `{method}`");
        }
        let response = serde_json::from_str::<SandboxJsonRpcResponse>(&line)?;
        if let Some(error) = response.error {
            anyhow::bail!(
                "sandbox proxy fixture `{method}` failed with code {}: {}",
                error.code,
                error.message
            );
        }
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    fn set_command_profile(&mut self, command_profile: &str) {
        let result = self
            .call(
                "policy.set_command_profile",
                json!({ "command_profile": command_profile }),
            )
            .expect("sandbox proxy policy update should succeed");
        assert_eq!(result["command_profile"], json!(command_profile));
    }

    fn exec_command(
        &mut self,
        thread_id: &str,
        request: CommandExecutionRequest,
    ) -> CommandExecutionResult {
        serde_json::from_value(
            self.call(
                "command.exec",
                json!({
                    "thread_id": thread_id,
                    "request": request,
                }),
            )
            .expect("sandbox proxy command.exec should succeed"),
        )
        .expect("sandbox proxy command.exec result should decode")
    }

    fn write_stdin(
        &mut self,
        thread_id: &str,
        request: CommandWriteRequest,
    ) -> CommandExecutionResult {
        serde_json::from_value(
            self.call(
                "command.write_stdin",
                json!({
                    "thread_id": thread_id,
                    "request": request,
                }),
            )
            .expect("sandbox proxy command.write_stdin should succeed"),
        )
        .expect("sandbox proxy command.write_stdin result should decode")
    }

    fn list_unread_tasks(&mut self, thread_id: &str) -> Vec<CommandTaskSnapshot> {
        serde_json::from_value(
            self.call(
                "command.list_unread_tasks",
                json!({ "thread_id": thread_id }),
            )
            .expect("sandbox proxy command.list_unread_tasks should succeed"),
        )
        .expect("sandbox proxy command.list_unread_tasks result should decode")
    }
}

impl Drop for SandboxProxyFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn repo_fake_bwrap_wrapper() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake-bwrap.sh")
}

#[cfg(unix)]
fn install_helper_link(source: &Path, target: &Path) {
    use std::os::unix::fs::symlink;

    let _ = fs::remove_file(target);
    symlink(source, target).unwrap_or_else(|error| {
        panic!(
            "failed to symlink helper `{}` -> `{}`: {error}",
            target.display(),
            source.display()
        )
    });
}

#[cfg(not(unix))]
fn install_helper_link(source: &Path, target: &Path) {
    fs::copy(source, target).unwrap_or_else(|error| {
        panic!(
            "failed to copy helper `{}` -> `{}`: {error}",
            source.display(),
            target.display()
        )
    });
    make_executable(target);
}

fn bubblewrap_yaml(executable_path: &Path) -> String {
    bubblewrap_yaml_with_compatibility(executable_path, "default", false, false, false, 1)
}

fn strict_bubblewrap_yaml(executable_path: &Path, selected_profile: &str) -> String {
    bubblewrap_yaml_with_compatibility(executable_path, selected_profile, true, true, true, 1)
}

fn strict_proxy_enforcement_plan_json() -> String {
    json!({
        "namespace": {
            "user": true,
            "ipc": true,
            "pid": true,
            "uts": true,
            "net": true,
        },
        "compatibility": {
            "require_landlock": true,
            "min_landlock_abi": 1,
            "require_seccomp": true,
            "strict": true,
        },
        "proxy": {
            "baseline_seccomp_profile": "proxy-baseline-v1",
            "landlock_profile": "workspace-rpc",
        },
        "default_command_profile": "default",
        "command_profiles": {
            "default": {
                "name": "default",
                "landlock_profile": "command-default",
                "seccomp_profile": "command-default-v1",
                "compatibility": {
                    "require_landlock": true,
                    "min_landlock_abi": 1,
                    "require_seccomp": true,
                    "strict": true,
                }
            },
            "readonly": {
                "name": "readonly",
                "landlock_profile": "command-readonly",
                "seccomp_profile": "command-readonly-v1",
                "compatibility": {
                    "require_landlock": true,
                    "min_landlock_abi": 1,
                    "require_seccomp": true,
                    "strict": true,
                }
            }
        }
    })
    .to_string()
}

fn bubblewrap_yaml_with_compatibility(
    executable_path: &Path,
    selected_profile: &str,
    require_landlock: bool,
    require_seccomp: bool,
    strict: bool,
    min_landlock_abi: u8,
) -> String {
    format!(
        r#"
sandbox:
  backend: "bubblewrap"
  workspace_sync_dir: "."
  restricted_host_paths:
    - "~/.ssh"
    - "~/.gnupg"
    - "/etc"
    - "/proc"
    - "/sys"
    - "/dev"
  allow_parent_access: false
  bubblewrap:
    executable: "{}"
    namespaces:
      user: true
      ipc: true
      pid: true
      uts: true
      net: true
    baseline_seccomp_profile: "proxy-baseline-v1"
    proxy_landlock_profile: "workspace-rpc"
    command_profiles:
      selected_profile: "{}"
      profiles:
        default:
          landlock_profile: "command-default"
          seccomp_profile: "command-default-v1"
        readonly:
          landlock_profile: "command-readonly"
          seccomp_profile: "command-readonly-v1"
    compatibility:
      require_landlock: {}
      min_landlock_abi: {}
      require_seccomp: {}
      strict: {}
  docker: {{}}
"#,
        executable_path.display(),
        selected_profile,
        require_landlock,
        min_landlock_abi,
        require_seccomp,
        strict
    )
}

fn host_supports_real_bwrap() -> bool {
    cfg!(target_os = "linux")
        && Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
}

fn host_supports_fake_bwrap_kernel_enforcement() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let fixture = SandboxFixture::new("openjarvis-sandbox-fake-bwrap-enforcement-probe");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = match SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(&wrapper, "default"),
        fixture.root(),
    ) {
        Ok(config) => config,
        Err(_) => return false,
    };
    build_sandbox(config).is_ok()
}

fn host_supports_real_bwrap_kernel_enforcement() -> bool {
    if !host_supports_real_bwrap() {
        return false;
    }
    let fixture = SandboxFixture::new("openjarvis-sandbox-real-bwrap-enforcement-probe");
    let config = match SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(Path::new("bwrap"), "default"),
        fixture.root(),
    ) {
        Ok(config) => config,
        Err(_) => return false,
    };
    build_sandbox(config).is_ok()
}

const SECCOMP_PROBE_CASES: &[&str] = &[
    "open_tree_missing",
    "move_mount_invalid",
    "mount_setattr_invalid",
    "clone3_invalid",
];

fn seccomp_probe_source() -> &'static str {
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

fn compile_seccomp_probe(workspace_root: &Path) -> PathBuf {
    let source_path = workspace_root.join("seccomp-probe.rs");
    let binary_path = workspace_root.join("seccomp-probe");
    fs::write(&source_path, seccomp_probe_source())
        .expect("seccomp probe source should be written");
    let output = Command::new("rustc")
        .arg("--edition=2021")
        .arg(&source_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("seccomp probe should compile");
    if !output.status.success() {
        panic!(
            "failed to compile seccomp probe: stdout=`{}` stderr=`{}`",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    binary_path
}

fn parse_seccomp_probe_errno_map(stdout: &[u8]) -> BTreeMap<String, i32> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let (case_name, errno_text) = line.split_once('=')?;
            Some((case_name.to_string(), errno_text.parse().ok()?))
        })
        .collect()
}

fn host_supported_seccomp_probe_cases(probe_binary: &Path) -> Vec<&'static str> {
    let output = Command::new(probe_binary)
        .args(SECCOMP_PROBE_CASES)
        .output()
        .expect("host seccomp probe should execute");
    let errno_map = parse_seccomp_probe_errno_map(&output.stdout);

    SECCOMP_PROBE_CASES
        .iter()
        .copied()
        .filter(|case_name| {
            errno_map
                .get(*case_name)
                .is_some_and(|errno| *errno != libc::EPERM && *errno != libc::ENOSYS)
        })
        .collect()
}

fn build_thread(thread_id: &str) -> Thread {
    Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", thread_id, thread_id),
        chrono::Utc::now(),
    )
}

fn drain_proxy_session(
    proxy: &mut SandboxProxyFixture,
    thread_id: &str,
    session_id: &str,
    first_chars: &str,
) -> CommandExecutionResult {
    let mut request = CommandWriteRequest::new(session_id.to_string());
    request.chars = first_chars.to_string();
    request.yield_time_ms = 300;
    let mut result = proxy.write_stdin(thread_id, request);
    for _ in 0..5 {
        if !result.running {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut poll = CommandWriteRequest::new(session_id.to_string());
        poll.yield_time_ms = 300;
        result = proxy.write_stdin(thread_id, poll);
    }
    result
}

#[test]
fn sandbox_capability_config_loads_from_workspace_file() {
    let fixture = SandboxFixture::new("openjarvis-sandbox-capability-config");
    fixture.write_capabilities_yaml(
        r#"
sandbox:
  backend: "disabled"
"#,
    );

    let config = SandboxCapabilityConfig::load_for_workspace(fixture.root())
        .expect("sandbox capability config should load");

    assert_eq!(config.sandbox().backend(), SandboxBackendKind::Disabled);
    assert_eq!(config.sandbox().workspace_sync_dir(), fixture.root());
    assert!(!config.sandbox().restricted_host_paths().is_empty());
}

#[test]
fn docker_sandbox_backend_returns_explicit_error() {
    let fixture = SandboxFixture::new("openjarvis-sandbox-docker-unimplemented");
    let config = SandboxCapabilityConfig::from_yaml_str(
        r#"
sandbox:
  backend: "docker"
"#,
        fixture.root(),
    )
    .expect("docker capability config should parse");

    let error = match build_sandbox(config) {
        Ok(_) => panic!("docker backend should be unimplemented"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("docker sandbox backend is not implemented")
    );
}

#[test]
fn bubblewrap_sandbox_rejects_parent_escape_and_restricted_paths() {
    let fixture = SandboxFixture::new("openjarvis-sandbox-path-policy");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("bubblewrap sandbox should build");

    let parent_error = sandbox
        .write_workspace_text(Path::new("../escape.txt"), "nope")
        .expect_err("parent escape should be rejected");
    assert!(
        parent_error
            .to_string()
            .contains("escapes synchronized workspace")
    );

    let restricted_error = sandbox
        .read_workspace_text(Path::new("/etc/passwd"))
        .expect_err("restricted absolute path should be rejected");
    assert!(
        restricted_error
            .to_string()
            .contains("restricted host directory")
    );
}

#[test]
fn bubblewrap_sandbox_allows_workspace_alias_and_tmp_paths() {
    // 测试场景: agent 在 sandbox 中看到的 `/workspace/...` 路径和显式 `/tmp/...` 路径都应可直接复用。
    let fixture = SandboxFixture::new("openjarvis-sandbox-path-aliases");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("bubblewrap sandbox should build");

    sandbox
        .write_workspace_text(Path::new("/workspace/alias/demo.txt"), "alias ok")
        .expect("workspace alias path should be writable");
    assert_eq!(
        fs::read_to_string(fixture.root().join("alias/demo.txt"))
            .expect("workspace alias write should be host visible"),
        "alias ok"
    );
    assert_eq!(
        sandbox
            .read_workspace_text(Path::new("/workspace/alias/demo.txt"))
            .expect("workspace alias path should be readable"),
        "alias ok"
    );

    let tmp_path =
        std::env::temp_dir().join(format!("openjarvis-sandbox-tmp-{}.txt", Uuid::new_v4()));
    sandbox
        .write_workspace_text(&tmp_path, "tmp ok")
        .expect("/tmp path should be writable");
    assert_eq!(
        fs::read_to_string(&tmp_path).expect("/tmp write should be host visible"),
        "tmp ok"
    );
    assert_eq!(
        sandbox
            .read_workspace_text(&tmp_path)
            .expect("/tmp path should be readable"),
        "tmp ok"
    );
    let _ = fs::remove_file(&tmp_path);
}

#[test]
fn bubblewrap_sandbox_jsonrpc_workspace_writes_are_host_visible() {
    let fixture = SandboxFixture::new("openjarvis-sandbox-jsonrpc-sync");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("bubblewrap sandbox should build");

    sandbox
        .write_workspace_text(Path::new("nested/demo.txt"), "hello sandbox")
        .expect("sandbox write should succeed");
    let host_path = fixture.root().join("nested/demo.txt");
    let host_content = fs::read_to_string(&host_path).expect("host workspace file should exist");

    assert_eq!(host_content, "hello sandbox");
    assert_eq!(
        sandbox
            .read_workspace_text(Path::new("nested/demo.txt"))
            .expect("sandbox read should succeed"),
        "hello sandbox"
    );
}

#[test]
fn bubblewrap_sandbox_real_bwrap_jsonrpc_workspace_writes_are_host_visible() {
    // 验证本机真实 bwrap 可以拉起 internal-sandbox proxy，并把 workspace 写入同步回宿主机。
    if !host_supports_real_bwrap() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-real-bwrap-jsonrpc-sync");
    let config = SandboxCapabilityConfig::from_yaml_str(
        &bubblewrap_yaml(Path::new("bwrap")),
        fixture.root(),
    )
    .expect("real bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("real bubblewrap sandbox should build");

    sandbox
        .write_workspace_text(Path::new("real/nested/demo.txt"), "hello real bwrap")
        .expect("real bubblewrap sandbox write should succeed");
    let host_path = fixture.root().join("real/nested/demo.txt");
    let host_content = fs::read_to_string(&host_path).expect("host workspace file should exist");

    assert_eq!(host_content, "hello real bwrap");
    assert_eq!(
        sandbox
            .read_workspace_text(Path::new("real/nested/demo.txt"))
            .expect("real bubblewrap sandbox read should succeed"),
        "hello real bwrap"
    );
}

#[test]
fn agent_worker_uses_configured_bubblewrap_sandbox_backend() {
    let fixture = SandboxFixture::new("openjarvis-sandbox-worker-backend");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");

    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("sandbox-ok")))
        .sandbox_capabilities(config)
        .build()
        .expect("worker should build with bubblewrap sandbox");

    assert_eq!(worker.sandbox().kind(), "bubblewrap");
    assert_eq!(worker.sandbox().workspace_root(), fixture.root());
}

#[tokio::test]
async fn sandbox_file_tools_route_through_proxy_into_workspace() {
    // 测试场景: sandbox 开启后，read/write/edit 应通过 proxy 读写当前工作区根，而不是额外落到 `.openjarvis/workspace`。
    let fixture = SandboxFixture::new("openjarvis-sandbox-tool-file-routing");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-file-tools");
    let relative_path = format!("tool/{}.txt", Uuid::new_v4());

    let write_result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "write".to_string(),
                arguments: json!({
                    "path": relative_path,
                    "content": "hello sandbox tool",
                }),
            },
        )
        .await
        .expect("sandbox write should succeed");
    assert!(write_result.content.contains("wrote"));

    let read_result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "read".to_string(),
                arguments: json!({
                    "path": relative_path,
                }),
            },
        )
        .await
        .expect("sandbox read should succeed");
    assert_eq!(read_result.content, "hello sandbox tool");

    registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "edit".to_string(),
                arguments: json!({
                    "path": relative_path,
                    "old_text": "sandbox",
                    "new_text": "proxy",
                }),
            },
        )
        .await
        .expect("sandbox edit should succeed");

    let host_path = fixture.root().join(&relative_path);
    let host_content = fs::read_to_string(&host_path).expect("host workspace file should exist");
    assert_eq!(host_content, "hello proxy tool");
}

#[tokio::test]
async fn sandbox_exec_command_tool_real_bwrap_runs_inside_workspace() {
    // 测试场景: sandbox 开启后，exec_command 应通过真实 bwrap + proxy 在 /workspace 中执行，并把变更同步回宿主机。
    if !host_supports_real_bwrap() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-tool-command-routing");
    let config = SandboxCapabilityConfig::from_yaml_str(
        &bubblewrap_yaml(Path::new("bwrap")),
        fixture.root(),
    )
    .expect("real bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("real bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-command-tools");

    let result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": "printf 'from-command' > cmd.txt && pwd && printf '\\ncommand-ok'",
                    "yield_time_ms": 300,
                }),
            },
        )
        .await
        .expect("sandbox exec_command should succeed");

    assert!(result.content.contains("command-ok"));
    assert_eq!(
        fs::read_to_string(fixture.root().join("cmd.txt"))
            .expect("sandbox command should write host-visible file"),
        "from-command"
    );
}

#[tokio::test]
async fn sandbox_exec_command_tool_real_bwrap_allows_tmp_workdir() {
    // 测试场景: sandbox 开启后，exec_command 显式指定 `/tmp` 作为工作目录应在真实 bwrap 中可写。
    if !host_supports_real_bwrap() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-tool-command-tmp-workdir");
    let config = SandboxCapabilityConfig::from_yaml_str(
        &bubblewrap_yaml(Path::new("bwrap")),
        fixture.root(),
    )
    .expect("real bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("real bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-command-tools-tmp");
    let tmp_file_name = format!("openjarvis-sandbox-command-{}.txt", Uuid::new_v4());
    let tmp_file_path = std::env::temp_dir().join(&tmp_file_name);

    let result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": format!("printf 'from-tmp-command' > {tmp_file_name} && pwd && printf '\\ntmp-command-ok'"),
                    "workdir": "/tmp",
                    "yield_time_ms": 300,
                }),
            },
        )
        .await
        .expect("sandbox exec_command with /tmp workdir should succeed");

    assert!(result.content.contains("/tmp"));
    assert!(result.content.contains("tmp-command-ok"));
    assert_eq!(
        fs::read_to_string(&tmp_file_path)
            .expect("sandbox command should write host-visible /tmp file"),
        "from-tmp-command"
    );
    let _ = fs::remove_file(&tmp_file_path);
}

#[tokio::test]
async fn sandbox_command_session_tools_route_through_proxy() {
    // 测试场景: sandbox 开启后，exec_command / list_unread_command_tasks / write_stdin 应共用 proxy 内的 command session。
    let fixture = SandboxFixture::new("openjarvis-sandbox-command-session-routing");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(&bubblewrap_yaml(&wrapper), fixture.root())
        .expect("bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-command-session-tools");

    let started = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": "sleep 0.05; printf 'proxy-finished'",
                    "yield_time_ms": 10,
                }),
            },
        )
        .await
        .expect("sandbox exec_command should start");
    let session_id = started.metadata["session_id"]
        .as_str()
        .expect("sandbox exec_command should expose session id")
        .to_string();
    assert!(started.content.contains("Process running with session ID"));

    sleep(Duration::from_millis(160)).await;

    let unread = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "list_unread_command_tasks".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("sandbox list_unread_command_tasks should succeed");
    assert!(unread.content.contains(&session_id));

    // 测试场景: session executor 可能需要多一次轮询才能把最终退出事件与尾部输出一起刷出来。
    let mut drained = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "write_stdin".to_string(),
                arguments: json!({
                    "session_id": session_id,
                    "chars": "",
                    "yield_time_ms": 200,
                }),
            },
        )
        .await
        .expect("sandbox write_stdin should drain proxy-owned session");
    for _ in 0..5 {
        if drained.content.contains("Process exited with code 0") {
            break;
        }
        sleep(Duration::from_millis(50)).await;
        drained = registry
            .call_for_context(
                &mut thread,
                ToolCallRequest {
                    name: "write_stdin".to_string(),
                    arguments: json!({
                        "session_id": session_id,
                        "chars": "",
                        "yield_time_ms": 200,
                    }),
                },
            )
            .await
            .expect("sandbox write_stdin should eventually observe session exit");
    }
    assert!(drained.content.contains("Process exited with code 0"));
    assert!(drained.content.contains("proxy-finished"));
}

#[test]
fn sandbox_proxy_policy_updates_only_affect_future_command_executors() {
    // 测试场景: proxy 更新 command profile 后，只影响后续新 executor；已完成的旧 executor 结果保持不变。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture =
        SandboxFixture::new_under_target("openjarvis-sandbox-proxy-policy-future-executors");
    let mut proxy = SandboxProxyFixture::spawn(fixture.root());
    let thread_id = "sandbox-policy-future";
    let mut initial_request =
        CommandExecutionRequest::new("printf 'before-update' > before-update.txt");
    initial_request.workdir = Some(fixture.root().to_path_buf());

    let initial = proxy.exec_command(thread_id, initial_request);
    assert_eq!(initial.exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(fixture.root().join("before-update.txt"))
            .expect("default profile write should succeed before policy update"),
        "before-update"
    );

    proxy.set_command_profile("readonly");

    let mut denied_request =
        CommandExecutionRequest::new("printf 'after-update' > after-update.txt");
    denied_request.workdir = Some(fixture.root().to_path_buf());
    let denied = proxy.exec_command(thread_id, denied_request);
    assert_ne!(
        denied.exit_code,
        Some(0),
        "future executor should adopt readonly profile after policy update"
    );
    assert!(
        !fixture.root().join("after-update.txt").exists(),
        "readonly future executor should fail closed on workspace writes"
    );
}

#[test]
fn sandbox_proxy_running_session_requires_rebuild_for_new_permissions() {
    // 测试场景: 运行中的 session 保留启动时快照；更新 profile 后必须重建 session 才能拿到新权限。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new_under_target("openjarvis-sandbox-proxy-session-rebuild");
    let mut proxy = SandboxProxyFixture::spawn(fixture.root());
    let thread_id = "sandbox-session-rebuild";
    proxy.set_command_profile("readonly");

    let mut readonly_request = CommandExecutionRequest::new(
        "IFS= read -r line; printf '%s' \"$line\" > stale-session.txt",
    );
    readonly_request.yield_time_ms = 10;
    readonly_request.workdir = Some(fixture.root().to_path_buf());
    let readonly_started = proxy.exec_command(thread_id, readonly_request);
    let readonly_session_id = readonly_started
        .session_id
        .clone()
        .expect("readonly command should start a session");
    assert!(readonly_started.running);

    proxy.set_command_profile("default");

    let readonly_result =
        drain_proxy_session(&mut proxy, thread_id, &readonly_session_id, "stale\n");
    assert_ne!(
        readonly_result.exit_code,
        Some(0),
        "running readonly session should not gain new permissions after profile update"
    );
    assert!(
        !fixture.root().join("stale-session.txt").exists(),
        "stale session should still be unable to write workspace content"
    );

    let mut rebuilt_request = CommandExecutionRequest::new(
        "IFS= read -r line; printf '%s' \"$line\" > rebuilt-session.txt",
    );
    rebuilt_request.yield_time_ms = 10;
    rebuilt_request.workdir = Some(fixture.root().to_path_buf());
    let rebuilt_started = proxy.exec_command(thread_id, rebuilt_request);
    let rebuilt_session_id = rebuilt_started
        .session_id
        .clone()
        .expect("rebuilt command should start a session");
    assert!(rebuilt_started.running);

    let rebuilt_result = drain_proxy_session(&mut proxy, thread_id, &rebuilt_session_id, "fresh\n");
    assert_eq!(
        rebuilt_result.exit_code,
        Some(0),
        "rebuilt session should pick up the new writable profile"
    );
    assert_eq!(
        fs::read_to_string(fixture.root().join("rebuilt-session.txt"))
            .expect("rebuilt session should write after profile update"),
        "fresh"
    );
}

#[test]
fn sandbox_proxy_rejects_unknown_command_profile_without_mutating_policy_source() {
    // 测试场景: 非法 profile 更新应显式失败，且 proxy 仍保留最近一次有效的 executor 策略来源。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new_under_target("openjarvis-sandbox-proxy-invalid-profile");
    let mut proxy = SandboxProxyFixture::spawn(fixture.root());
    proxy.set_command_profile("readonly");

    let error = proxy
        .call(
            "policy.set_command_profile",
            json!({ "command_profile": "missing-profile" }),
        )
        .expect_err("unknown command profile should fail");
    assert!(
        error
            .to_string()
            .contains("unknown sandbox command profile `missing-profile`"),
        "unexpected sandbox proxy profile error: {error:#}"
    );

    let mut denied_request = CommandExecutionRequest::new("printf 'blocked' > blocked.txt");
    denied_request.workdir = Some(fixture.root().to_path_buf());
    let denied = proxy.exec_command("sandbox-invalid-profile", denied_request);
    assert_ne!(
        denied.exit_code,
        Some(0),
        "failed profile update must not silently widen the existing readonly policy"
    );
    assert!(
        !fixture.root().join("blocked.txt").exists(),
        "readonly policy source should remain active after a rejected profile update"
    );
}

#[test]
fn sandbox_proxy_session_executor_enforces_thread_isolation() {
    // 测试场景: sandbox proxy 只向所属线程暴露 unread session，并在跨线程续写失败后保留原 session。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new_under_target("openjarvis-sandbox-proxy-thread-isolation");
    let mut proxy = SandboxProxyFixture::spawn(fixture.root());

    let mut owner_request = CommandExecutionRequest::new("sleep 0.05; printf 'owner-output'");
    owner_request.yield_time_ms = 10;
    let owner_started = proxy.exec_command("thread-owner", owner_request);
    let owner_session_id = owner_started
        .session_id
        .clone()
        .expect("owner thread command should expose a session id");
    assert!(owner_started.running);

    let mut other_request = CommandExecutionRequest::new("sleep 0.05; printf 'other-output'");
    other_request.yield_time_ms = 10;
    let other_started = proxy.exec_command("thread-other", other_request);
    let other_session_id = other_started
        .session_id
        .clone()
        .expect("other thread command should expose a session id");
    assert!(other_started.running);

    std::thread::sleep(std::time::Duration::from_millis(160));

    let owner_tasks = proxy.list_unread_tasks("thread-owner");
    assert_eq!(owner_tasks.len(), 1);
    assert_eq!(owner_tasks[0].thread_id, "thread-owner");
    assert_eq!(owner_tasks[0].session_id, owner_session_id);
    assert!(owner_tasks[0].has_unread_output);

    let other_tasks = proxy.list_unread_tasks("thread-other");
    assert_eq!(other_tasks.len(), 1);
    assert_eq!(other_tasks[0].thread_id, "thread-other");
    assert_eq!(other_tasks[0].session_id, other_session_id);
    assert!(other_tasks[0].has_unread_output);

    let error = proxy
        .call(
            "command.write_stdin",
            json!({
                "thread_id": "thread-other",
                "request": {
                    "session_id": owner_session_id,
                    "chars": "",
                    "yield_time_ms": 10,
                },
            }),
        )
        .expect_err("foreign thread should not access a sandbox session executor");
    assert!(
        error.to_string().contains("does not belong to thread"),
        "unexpected sandbox proxy cross-thread error: {error:#}"
    );

    let owner_tasks_after_error = proxy.list_unread_tasks("thread-owner");
    assert_eq!(owner_tasks_after_error.len(), 1);
    assert_eq!(owner_tasks_after_error[0].session_id, owner_session_id);
    assert!(owner_tasks_after_error[0].has_unread_output);

    let owner_result = drain_proxy_session(&mut proxy, "thread-owner", &owner_session_id, "");
    assert_eq!(owner_result.exit_code, Some(0));
    assert!(owner_result.output.contains("owner-output"));

    let other_result = drain_proxy_session(&mut proxy, "thread-other", &other_session_id, "");
    assert_eq!(other_result.exit_code, Some(0));
    assert!(other_result.output.contains("other-output"));
}

#[test]
fn sandbox_command_executor_does_not_leak_policy_snapshot_fd_to_final_command() {
    // 测试场景: executor 读完策略快照后必须关闭策略 fd，最终命令不应继续看到该控制面 fd。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new_under_target("openjarvis-sandbox-policy-fd-hygiene");
    let mut proxy = SandboxProxyFixture::spawn(fixture.root());

    let result = proxy.exec_command(
        "sandbox-policy-fd",
        CommandExecutionRequest::new(
            "if [ -e /proc/self/fd/3 ]; then printf 'leaked'; else printf 'clean'; fi",
        ),
    );
    assert_eq!(result.exit_code, Some(0));
    assert!(
        result.output.contains("clean"),
        "final command should not inherit sandbox policy fd 3"
    );
    assert!(
        !result.output.contains("leaked"),
        "sandbox policy fd should be closed before final command exec"
    );
}

#[tokio::test]
async fn sandbox_exec_command_tool_denies_escape_syscalls_from_new_mount_api() {
    // 测试场景: sandbox baseline/child seccomp 应直接拒绝新 mount API 与 clone3 等逃逸导向 syscall。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-seccomp-escape-deny");
    let probe_binary = compile_seccomp_probe(fixture.root());
    let supported_cases = host_supported_seccomp_probe_cases(&probe_binary);
    if supported_cases.is_empty() {
        return;
    }

    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(&wrapper, "default"),
        fixture.root(),
    )
    .expect("strict bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("strict bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-seccomp-escape-deny");

    let result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": format!("{} {}", probe_binary.display(), supported_cases.join(" ")),
                    "yield_time_ms": 1_000,
                }),
            },
        )
        .await
        .expect("sandbox seccomp probe should return a structured result");

    assert_eq!(result.metadata["exit_code"].as_i64(), Some(0));
    for case_name in &supported_cases {
        assert!(
            result
                .content
                .contains(&format!("{case_name}={}", libc::EPERM)),
            "sandbox seccomp probe should deny `{case_name}` with EPERM"
        );
    }
}

#[test]
fn bubblewrap_sandbox_strict_kernel_enforcement_boots_proxy_and_syncs_workspace() {
    // 测试场景: 严格 enforcement 可满足时，proxy 应先完成收口再对外提供正常的 workspace 同步能力。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-strict-proxy-enforcement");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(&wrapper, "default"),
        fixture.root(),
    )
    .expect("strict bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("strict bubblewrap sandbox should build");

    sandbox
        .write_workspace_text(Path::new("strict/proxy.txt"), "proxy-enforced")
        .expect("strict proxy should still allow workspace writes");
    assert_eq!(
        fs::read_to_string(fixture.root().join("strict/proxy.txt"))
            .expect("strict proxy write should be host visible"),
        "proxy-enforced"
    );
}

#[test]
fn bubblewrap_sandbox_real_bwrap_strict_kernel_enforcement_syncs_workspace() {
    // 测试场景: 真实 bwrap 满足严格 enforcement 时，proxy 仍应成功握手并保留 workspace 同步语义。
    if !host_supports_real_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-real-bwrap-strict-enforcement");
    let config = SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(Path::new("bwrap"), "default"),
        fixture.root(),
    )
    .expect("strict real bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("strict real bubblewrap sandbox should build");

    sandbox
        .write_workspace_text(Path::new("strict/real-proxy.txt"), "real-proxy-enforced")
        .expect("strict real proxy should allow workspace writes");
    assert_eq!(
        fs::read_to_string(fixture.root().join("strict/real-proxy.txt"))
            .expect("strict real proxy write should be host visible"),
        "real-proxy-enforced"
    );
}

#[tokio::test]
async fn sandbox_command_child_helper_enforces_readonly_profile_for_pipe_and_pty() {
    // 测试场景: child helper 应在 pipe/PTY 两条链路里都安装 readonly profile，并拒绝写入 workspace。
    if !host_supports_fake_bwrap_kernel_enforcement() {
        return;
    }

    let fixture = SandboxFixture::new("openjarvis-sandbox-child-helper-readonly");
    let wrapper = fixture.install_fake_bwrap_wrapper();
    let config = SandboxCapabilityConfig::from_yaml_str(
        &strict_bubblewrap_yaml(&wrapper, "readonly"),
        fixture.root(),
    )
    .expect("readonly bubblewrap capability config should parse");
    let sandbox = build_sandbox(config).expect("readonly bubblewrap sandbox should build");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry.install_sandbox(Arc::clone(&sandbox));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread = build_thread("sandbox-child-helper-readonly");

    let pipe_result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": "printf 'blocked' > readonly-pipe.txt",
                    "yield_time_ms": 1_000,
                }),
            },
        )
        .await
        .expect("readonly pipe command should return a structured result");
    assert_ne!(pipe_result.metadata["exit_code"].as_i64(), Some(0));
    assert!(
        !fixture.root().join("readonly-pipe.txt").exists(),
        "readonly pipe profile should block workspace writes"
    );

    let pty_result = registry
        .call_for_context(
            &mut thread,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": "printf 'blocked' > readonly-pty.txt",
                    "tty": true,
                    "yield_time_ms": 1_000,
                }),
            },
        )
        .await
        .expect("readonly PTY command should return a structured result");
    assert_ne!(pty_result.metadata["exit_code"].as_i64(), Some(0));
    assert!(
        !fixture.root().join("readonly-pty.txt").exists(),
        "readonly PTY profile should block workspace writes"
    );
}
