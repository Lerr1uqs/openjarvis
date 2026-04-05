use openjarvis::{
    agent::{AgentWorker, SkillManifest},
    config::{AppConfig, install_global_config},
    router::ChannelRouter,
};
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

struct MainConfigFixture {
    root: PathBuf,
    config_path: PathBuf,
}

impl MainConfigFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let config_path = root.join("config.yaml");
        Self { root, config_path }
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write_yaml(&self, yaml: &str) {
        fs::write(&self.config_path, yaml).expect("fixture yaml should be written");
    }

    fn write_raw_mcp_json(&self, raw: &str) {
        let mcp_json_path = self.root.join("config/openjarvis/mcp.json");
        fs::create_dir_all(
            mcp_json_path
                .parent()
                .expect("mcp json parent path should exist"),
        )
        .expect("mcp json directory should be created");
        fs::write(&mcp_json_path, raw).expect("raw mcp json should be written");
    }
}

impl Drop for MainConfigFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn acpx_skill_resource_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/unittest/skills/acpx/SKILL.md")
}

#[tokio::test]
async fn startup_components_build_from_default_config() {
    let config = AppConfig::default();
    let agent = AgentWorker::from_config(&config)
        .await
        .expect("agent should build");
    let _router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
}

#[tokio::test]
async fn startup_components_build_from_installed_global_config() {
    let config = AppConfig::builder_for_test()
        .build()
        .expect("test config should validate");
    install_global_config(config).expect("global config should install");

    let agent = AgentWorker::from_global_config()
        .await
        .expect("agent should build from global config");
    let _router = ChannelRouter::builder()
        .agent(agent)
        .build()
        .expect("router should build");
}

#[test]
fn startup_exits_when_external_mcp_json_cannot_be_parsed() {
    let fixture = MainConfigFixture::new("openjarvis-main-invalid-mcp-json");
    fixture.write_yaml(
        r#"
llm:
  provider: "mock"
"#,
    );
    fixture.write_raw_mcp_json(
        r#"
{ "mcpServers": { "broken": { "command": "openjarvis", } } }
"#,
    );

    // Requirement: if the external MCP sidecar cannot be parsed, startup must fail instead of
    // silently continuing with a broken MCP configuration.
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .env("RUST_LOG", "info")
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to parse mcp config file"));
    assert!(stderr.contains("config/openjarvis/mcp.json"));
}

#[test]
fn skill_install_command_rejects_unknown_curated_skill_before_app_config_load() {
    let fixture = MainConfigFixture::new("openjarvis-main-skill-install-before-config");
    fixture.write_yaml(":\ninvalid\n");

    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("skill")
        .arg("install")
        .arg("missing-skill")
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .current_dir(fixture.root())
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported curated skill `missing-skill`"));
    assert!(!stderr.contains("failed to parse config file"));
}

#[test]
fn skill_install_and_uninstall_commands_manage_workspace_skill_files() {
    let fixture = MainConfigFixture::new("openjarvis-main-skill-install-uninstall");
    let install_output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("skill")
        .arg("install")
        .arg("acpx")
        .env(
            "OPENJARVIS_CURATED_SKILL_ACPX_PATH",
            acpx_skill_resource_path(),
        )
        .current_dir(fixture.root())
        .output()
        .expect("openjarvis binary should run skill install");

    assert!(install_output.status.success());
    let skill_file = fixture.root().join(".openjarvis/skills/acpx/SKILL.md");
    assert!(skill_file.exists());
    assert_eq!(
        skill_file.file_name().and_then(|name| name.to_str()),
        Some("SKILL.md")
    );

    let manifest =
        SkillManifest::from_skill_file(&skill_file).expect("installed acpx skill should parse");
    assert_eq!(manifest.name, "acpx");
    assert!(
        manifest
            .description
            .contains("agent-to-agent communication")
    );

    let uninstall_output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("skill")
        .arg("uninstall")
        .arg("acpx")
        .current_dir(fixture.root())
        .output()
        .expect("openjarvis binary should run skill uninstall");

    assert!(uninstall_output.status.success());
    assert!(!skill_file.exists());
    assert!(!fixture.root().join(".openjarvis/skills/acpx").exists());
}

#[test]
fn startup_exits_when_test_only_load_skill_target_is_missing() {
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("--load-skill")
        .arg("missing_local_skill")
        .env("RUST_LOG", "info")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("local skill `missing_local_skill` does not exist"));
}

#[test]
fn cargo_manifest_sets_default_run_to_openjarvis() {
    // 验证场景: 仓库存在多个二进制目标时, `cargo run -- ...` 仍应默认落到 openjarvis。
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest =
        fs::read_to_string(&manifest_path).expect("Cargo.toml should be readable for assertions");

    assert!(manifest.contains("default-run = \"openjarvis\""));
}

#[test]
fn internal_browser_helper_runs_before_app_config_load_and_reports_spawn_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("internal-browser")
        .arg("smoke")
        .arg("--url")
        .arg("https://example.com")
        .arg("--headless")
        .arg("--node-bin")
        .arg("missing-browser-node")
        .env("RUST_LOG", "info")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to spawn browser sidecar executable"));
}

#[test]
fn startup_writes_logs_to_local_file() {
    let fixture = MainConfigFixture::new("openjarvis-main-file-logging");
    fixture.write_yaml(
        r#"
logging:
  level: "info"
  stderr: false
  file:
    enabled: true
    directory: "local-logs"
    rotation: "never"
    filename_prefix: "openjarvis-test"
    filename_suffix: "log"
    max_files: 2
llm:
  provider: "mock"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("--load-skill")
        .arg("missing_local_skill")
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());

    let log_directory = fixture.root.join("local-logs");
    let log_entries = fs::read_dir(&log_directory)
        .expect("local log directory should be created")
        .collect::<Result<Vec<_>, _>>()
        .expect("local log directory entries should be readable");
    assert!(
        !log_entries.is_empty(),
        "expected at least one local log file"
    );

    let log_path = log_entries[0].path();
    let log_output = fs::read_to_string(&log_path).expect("local log file should be readable");

    assert!(log_path.file_name().is_some());
    assert!(log_output.contains("tracing initialized"));
    assert!(log_output.contains("mcp sidecar config not found"));
}

#[test]
fn startup_debug_flag_overrides_rust_log_and_enables_stderr_logs() {
    let fixture = MainConfigFixture::new("openjarvis-main-debug-cli-override");
    fixture.write_yaml(
        r#"
logging:
  level: "info"
  stderr: false
  file:
    enabled: true
    directory: "debug-logs"
    rotation: "never"
    filename_prefix: "openjarvis-debug"
    filename_suffix: "log"
    max_files: 1
llm:
  provider: "mock"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("--debug")
        .arg("--load-skill")
        .arg("missing_local_skill")
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .env("RUST_LOG", "info")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("DEBUG"));
    assert!(stderr.contains("debug logging enabled via CLI override"));
}

#[test]
fn startup_log_color_flag_emits_ansi_stderr_logs() {
    let fixture = MainConfigFixture::new("openjarvis-main-log-color-cli");
    fixture.write_yaml(
        r#"
logging:
  level: "info"
  stderr: false
  file:
    enabled: true
    directory: "color-logs"
    rotation: "never"
    filename_prefix: "openjarvis-color"
    filename_suffix: "log"
    max_files: 1
llm:
  provider: "mock"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("--log-color")
        .arg("--load-skill")
        .arg("missing_local_skill")
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\u{1b}["));
    assert!(stderr.contains("tracing initialized"));
}

#[test]
fn startup_does_not_emit_missing_log_directory_noise_on_first_run() {
    let fixture = MainConfigFixture::new("openjarvis-main-log-dir-bootstrap");
    fixture.write_yaml(
        r#"
logging:
  level: "info"
  stderr: true
  file:
    enabled: true
    directory: "first-run-logs"
    rotation: "never"
    filename_prefix: "openjarvis-first-run"
    filename_suffix: "log"
    max_files: 2
llm:
  provider: "mock"
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_openjarvis"))
        .arg("--load-skill")
        .arg("missing_local_skill")
        .env("OPENJARVIS_CONFIG", fixture.config_path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("openjarvis binary should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    // 验证场景: 首次启动且日志目录不存在时，不应再输出 tracing-appender 的目录扫描噪声。
    assert!(!stderr.contains("Error reading the log directory/files"));

    let log_directory = fixture.root.join("first-run-logs");
    assert!(log_directory.exists());
}
