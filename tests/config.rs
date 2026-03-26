use openjarvis::config::{
    AgentMcpServerTransportConfig, AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT,
};
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

struct ConfigFixture {
    root: PathBuf,
    config_path: PathBuf,
}

impl ConfigFixture {
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

    fn write_mcp_json(&self, value: serde_json::Value) {
        let mcp_json_path = self.root.join("config/openjarvis/mcp.json");
        fs::create_dir_all(
            mcp_json_path
                .parent()
                .expect("mcp json parent path should exist"),
        )
        .expect("mcp json directory should be created");
        fs::write(
            &mcp_json_path,
            serde_json::to_string_pretty(&value).expect("mcp json should serialize"),
        )
        .expect("mcp json should be written");
    }
}

impl Drop for ConfigFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Default)]
struct CapturedLogWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CapturedLogWriter {
    fn output(&self) -> String {
        String::from_utf8(
            self.buffer
                .lock()
                .expect("log buffer lock should succeed")
                .clone(),
        )
        .expect("captured logs should be utf-8")
    }
}

struct CapturedLogGuard {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturedLogGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer
            .lock()
            .expect("log buffer lock should succeed")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogWriter {
    type Writer = CapturedLogGuard;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogGuard {
            buffer: Arc::clone(&self.buffer),
        }
    }
}

#[test]
fn missing_config_path_returns_default_config() {
    let path = temp_dir().join(format!("openjarvis-missing-{}.yaml", Uuid::new_v4()));
    let config = AppConfig::from_path(&path).expect("missing path should fall back to defaults");

    assert_eq!(config.llm_config().provider, "mock");
    assert_eq!(
        config.channel_config().feishu_config().mode,
        "long_connection"
    );
}

#[test]
fn yaml_config_can_be_loaded_from_path() {
    let path = temp_dir().join(format!("openjarvis-config-{}.yaml", Uuid::new_v4()));
    fs::write(
        &path,
        r#"
feishu:
  mode: ""
llm:
  provider: "mock_llm"
  mock_response: "pong"
"#,
    )
    .expect("temp config should be written");

    let config = AppConfig::from_path(&path).expect("yaml config should parse");
    fs::remove_file(&path).expect("temp config should be removed");

    assert_eq!(config.channel_config().feishu_config().mode, "");
    assert_eq!(config.llm_config().provider, "mock_llm");
    assert_eq!(config.llm_config().mock_response, "pong");
}

#[test]
fn external_mcp_json_can_be_loaded_without_yaml_file() {
    let fixture = ConfigFixture::new("openjarvis-mcp-json-only");
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "demo_stdio_file": {
                "command": "openjarvis",
                "args": ["internal-mcp", "demo-stdio"],
                "env": {
                    "OPENJARVIS_DEMO": "1"
                }
            },
            "demo_http_file": {
                "enabled": false,
                "url": "http://127.0.0.1:39090/mcp"
            }
        }
    }));

    let config =
        AppConfig::from_path(fixture.config_path()).expect("external mcp json should load");
    let mcp_servers = config.agent_config().tool_config().mcp_config().servers();

    assert_eq!(mcp_servers.len(), 2);
    assert!(mcp_servers["demo_stdio_file"].enabled);
    assert!(!mcp_servers["demo_http_file"].enabled);

    match mcp_servers["demo_stdio_file"].transport_config() {
        AgentMcpServerTransportConfig::Stdio { command, args, env } => {
            assert_eq!(command, "openjarvis");
            assert_eq!(
                args,
                &["internal-mcp".to_string(), "demo-stdio".to_string()]
            );
            assert_eq!(env.get("OPENJARVIS_DEMO").map(String::as_str), Some("1"));
        }
        other => panic!("unexpected stdio transport: {other:?}"),
    }

    match mcp_servers["demo_http_file"].transport_config() {
        AgentMcpServerTransportConfig::StreamableHttp { url } => {
            assert_eq!(url, "http://127.0.0.1:39090/mcp");
        }
        other => panic!("unexpected http transport: {other:?}"),
    }
}

#[test]
fn external_mcp_json_merges_with_yaml_defined_mcp_servers() {
    let fixture = ConfigFixture::new("openjarvis-mcp-json-merge");
    fixture.write_yaml(
        r#"
agent:
  tool:
    mcp:
      servers:
        yaml_demo:
          enabled: true
          transport: stdio
          command: "openjarvis"
          args: ["internal-mcp", "demo-stdio"]
llm:
  provider: "mock"
"#,
    );
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "json_demo": {
                "command": "openjarvis",
                "args": ["internal-mcp", "demo-stdio"]
            }
        }
    }));

    let config = AppConfig::from_path(fixture.config_path())
        .expect("yaml and external mcp json should merge");
    let mcp_servers = config.agent_config().tool_config().mcp_config().servers();

    assert_eq!(mcp_servers.len(), 2);
    assert!(mcp_servers.contains_key("yaml_demo"));
    assert!(mcp_servers.contains_key("json_demo"));
}

#[test]
fn missing_external_mcp_json_logs_note_and_keeps_mcp_empty() {
    let fixture = ConfigFixture::new("openjarvis-mcp-json-missing-note");
    fixture.write_yaml(
        r#"
llm:
  provider: "mock"
"#,
    );

    let writer = CapturedLogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(false)
        .without_time()
        .with_max_level(Level::INFO)
        .finish();

    let config = tracing::subscriber::with_default(subscriber, || {
        AppConfig::from_path(fixture.config_path()).expect("missing mcp json should be allowed")
    });

    assert!(config.agent_config().tool_config().mcp_config().is_empty());

    let output = writer.output();
    if !output.is_empty() {
        assert!(output.contains("mcp sidecar config not found"));
        assert!(output.contains("continuing without external MCP servers"));
    }
}

#[test]
fn default_assistant_system_prompt_is_not_empty() {
    assert!(!DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim().is_empty());
}

#[test]
fn default_agent_hook_config_is_empty() {
    let config = AppConfig::default();

    assert!(config.agent_config().hook_config().is_empty());
}

#[test]
fn malformed_hook_config_with_unknown_event_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-unknown-event-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    not_a_real_event: ["echo", "hello"]
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("unknown hook event should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("unknown field `not_a_real_event`"));
}

#[test]
fn malformed_hook_config_with_empty_command_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-empty-command-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    notification: []
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("empty hook command should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("notification hook command must not be empty"));
}

#[test]
fn malformed_hook_config_with_blank_command_part_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-blank-part-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook:
    notification: ["powershell", "   "]
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error =
        AppConfig::from_path(&path).expect_err("blank hook command part should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("notification hook command part at index 1 must not be blank"));
}

#[test]
fn malformed_hook_config_with_wrong_type_is_rejected() {
    let path = temp_dir().join(format!(
        "openjarvis-hook-wrong-type-{}.yaml",
        Uuid::new_v4()
    ));
    fs::write(
        &path,
        r#"
agent:
  hook: "invalid"
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error = AppConfig::from_path(&path).expect_err("invalid hook section should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("invalid type"));
}

#[test]
fn agent_tool_mcp_config_parses_stdio_and_streamable_http_servers() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  tool:
    mcp:
      servers:
        demo_stdio:
          enabled: false
          transport: stdio
          command: "openjarvis"
          args: ["internal-mcp", "demo-stdio"]
        demo_http:
          enabled: true
          transport: http
          url: "http://127.0.0.1:39090/mcp"
llm:
  provider: "mock"
"#,
    )
    .expect("mcp config should parse");

    let mcp_servers = config.agent_config().tool_config().mcp_config().servers();
    assert_eq!(mcp_servers.len(), 2);
    assert!(!mcp_servers["demo_stdio"].enabled);
    assert!(mcp_servers["demo_http"].enabled);
}

#[test]
fn malformed_mcp_config_with_blank_streamable_http_url_is_rejected() {
    let path = temp_dir().join(format!("openjarvis-mcp-blank-url-{}.yaml", Uuid::new_v4()));
    fs::write(
        &path,
        r#"
agent:
  tool:
    mcp:
      servers:
        bad_http:
          transport: streamable_http
          url: "   "
llm:
  provider: "mock"
"#,
    )
    .expect("temp config should be written");

    let error =
        AppConfig::from_path(&path).expect_err("blank streamable_http MCP url should be rejected");
    fs::remove_file(&path).expect("temp config should be removed");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("mcp server `bad_http` streamable_http url must not be blank"));
}

#[test]
fn malformed_external_mcp_json_with_duplicate_server_name_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-mcp-json-duplicate");
    fixture.write_yaml(
        r#"
agent:
  tool:
    mcp:
      servers:
        duplicate_demo:
          transport: stdio
          command: "openjarvis"
          args: ["internal-mcp", "demo-stdio"]
llm:
  provider: "mock"
"#,
    );
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "duplicate_demo": {
                "command": "openjarvis",
                "args": ["internal-mcp", "demo-stdio"]
            }
        }
    }));

    let error = AppConfig::from_path(fixture.config_path())
        .expect_err("duplicate mcp server names should be rejected");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("mcp server `duplicate_demo` is defined in both YAML config"));
    assert!(error_chain.contains("config/openjarvis/mcp.json"));
}

#[test]
fn malformed_external_mcp_json_with_ambiguous_transport_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-mcp-json-ambiguous");
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "bad_demo": {
                "command": "openjarvis",
                "url": "http://127.0.0.1:39090/mcp"
            }
        }
    }));

    let error = AppConfig::from_path(fixture.config_path())
        .expect_err("ambiguous external mcp server should be rejected");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("failed to validate mcp config file"));
    assert!(
        error_chain
            .contains("mcp.json server `bad_demo` must define either `command` or `url`, not both")
    );
}
