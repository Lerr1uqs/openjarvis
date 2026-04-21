use openjarvis::{
    agent::{
        AgentWorker, SandboxBackendKind, SandboxCapabilityConfig, ToolCallRequest, ToolRegistry,
        build_sandbox,
    },
    llm::MockLLMProvider,
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
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

fn build_thread(thread_id: &str) -> Thread {
    Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", thread_id, thread_id),
        chrono::Utc::now(),
    )
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

    let drained = registry
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
    assert!(drained.content.contains("Process exited with code 0"));
    assert!(drained.content.contains("proxy-finished"));
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
