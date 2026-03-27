use openjarvis::{agent::AgentWorker, config::AppConfig, router::ChannelRouter};
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
