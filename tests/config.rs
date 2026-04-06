use openjarvis::config::{
    AgentMcpServerTransportConfig, AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, LLMConfig,
    LogRotation, SessionStoreBackend, global_config, install_global_config, try_global_config,
};
use serde_json::json;
use std::{
    any::Any,
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

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

#[test]
fn missing_config_path_returns_default_config() {
    let path = temp_dir().join(format!("openjarvis-missing-{}.yaml", Uuid::new_v4()));
    let config =
        AppConfig::from_yaml_path(&path).expect("missing path should fall back to defaults");

    assert_eq!(config.llm_config().provider, "unknown");
    assert_eq!(config.llm_config().effective_protocol(), "mock");
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
  protocol: "mock"
  provider: "mock_llm"
  mock_response: "pong"
"#,
    )
    .expect("temp config should be written");

    let config = AppConfig::from_yaml_path(&path).expect("yaml config should parse");
    fs::remove_file(&path).expect("temp config should be removed");

    assert_eq!(config.channel_config().feishu_config().mode, "");
    assert_eq!(config.llm_config().provider, "mock_llm");
    assert_eq!(config.llm_config().effective_protocol(), "mock");
    assert_eq!(config.llm_config().mock_response, "pong");
}

#[test]
fn yaml_config_can_be_loaded_from_string() {
    let config = AppConfig::from_yaml_str(
        r#"
llm:
  protocol: "mock"
  provider: "mock_llm"
  mock_response: "from-yaml-str"
"#,
    )
    .expect("yaml string should parse");

    assert_eq!(config.llm_config().provider, "mock_llm");
    assert_eq!(config.llm_config().effective_protocol(), "mock");
    assert_eq!(config.llm_config().mock_response, "from-yaml-str");
}

#[test]
fn builder_for_test_builds_minimal_validated_config() {
    let config = AppConfig::builder_for_test()
        .llm(LLMConfig {
            protocol: "mock".to_string(),
            mock_response: "builder-mock".to_string(),
            ..LLMConfig::default()
        })
        .build()
        .expect("builder config should validate");

    assert_eq!(config.llm_config().provider, "unknown");
    assert_eq!(config.llm_config().effective_protocol(), "mock");
    assert_eq!(config.llm_config().mock_response, "builder-mock");
}

#[test]
fn global_config_installation_is_single_assignment_and_fail_fast() {
    assert!(try_global_config().is_none());

    let panic = std::panic::catch_unwind(global_config)
        .expect_err("access before install should fail fast");
    assert!(panic_message(panic).contains("global app config is not installed"));

    let installed = install_global_config(
        AppConfig::builder_for_test()
            .llm(LLMConfig {
                protocol: "mock".to_string(),
                mock_response: "installed".to_string(),
                ..LLMConfig::default()
            })
            .build()
            .expect("builder config should validate"),
    )
    .expect("first install should succeed");
    assert_eq!(installed.llm_config().mock_response, "installed");
    assert!(try_global_config().is_some());
    assert_eq!(global_config().llm_config().mock_response, "installed");

    let duplicate_error = install_global_config(
        AppConfig::builder_for_test()
            .build()
            .expect("default builder config should validate"),
    )
    .expect_err("duplicate install should fail");
    assert!(
        duplicate_error
            .to_string()
            .contains("already been installed")
    );
}

#[test]
fn yaml_llm_provider_without_protocol_is_rejected() {
    let error = AppConfig::from_yaml_str(
        r#"
llm:
  provider: "zai"
  model: "glm-5"
  base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
  api_key: "test-key"
"#,
    )
    .expect_err("provider-only llm config should be rejected");

    assert!(format!("{error:#}").contains("llm.protocol is required"));
}

#[test]
fn yaml_llm_legacy_protocal_key_is_rejected() {
    let error = AppConfig::from_yaml_str(
        r#"
llm:
  protocal: "openai"
  provider: "zai"
  model: "glm-5"
  base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
  api_key: "test-key"
"#,
    )
    .expect_err("legacy protocal key should be rejected");

    assert!(format!("{error:#}").contains("llm.protocol is required"));
}

#[test]
fn default_logging_config_enables_local_file_sink() {
    let config = AppConfig::default();

    assert_eq!(config.logging_config().level_filter(), "info");
    assert!(config.logging_config().stderr_enabled());
    assert!(config.logging_config().file_config().enabled());
    assert_eq!(
        config.logging_config().file_config().directory(),
        Path::new("logs")
    );
    assert_eq!(
        config.logging_config().file_config().rotation(),
        LogRotation::Daily
    );
    assert_eq!(
        config.session_config().persistence_config().backend(),
        SessionStoreBackend::Sqlite
    );
}

#[test]
fn session_sqlite_path_resolves_relative_to_config_root() {
    let fixture = ConfigFixture::new("openjarvis-session-sqlite-relative-path");
    fixture.write_yaml(
        r#"
session:
  persistence:
    backend: "sqlite"
    sqlite:
      path: "runtime/session.sqlite3"
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let config =
        AppConfig::from_path(fixture.config_path()).expect("session sqlite path should resolve");

    assert_eq!(
        config
            .session_config()
            .persistence_config()
            .sqlite_config()
            .path(),
        fixture.root.join("runtime/session.sqlite3").as_path()
    );
}

#[test]
fn session_memory_backend_parses_from_yaml() {
    let config = AppConfig::from_yaml_str(
        r#"
session:
  persistence:
    backend: "memory"
llm:
  protocol: "mock"
  provider: "mock"
"#,
    )
    .expect("memory session backend should parse");

    assert_eq!(
        config.session_config().persistence_config().backend(),
        SessionStoreBackend::Memory
    );
}

#[test]
fn logging_config_resolves_relative_directory_against_config_path() {
    let fixture = ConfigFixture::new("openjarvis-logging-relative-dir");
    fixture.write_yaml(
        r#"
logging:
  level: "debug"
  file:
    directory: "runtime-logs"
    rotation: "hourly"
    filename_prefix: "jarvis"
    filename_suffix: "txt"
    max_files: 3
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let config =
        AppConfig::from_path(fixture.config_path()).expect("logging config should resolve paths");
    let expected_directory = fixture.root.join("runtime-logs");

    assert_eq!(config.logging_config().level_filter(), "debug");
    assert_eq!(
        config.logging_config().file_config().directory(),
        expected_directory.as_path()
    );
    assert_eq!(
        config.logging_config().file_config().rotation(),
        LogRotation::Hourly
    );
    assert_eq!(
        config.logging_config().file_config().filename_prefix(),
        "jarvis"
    );
    assert_eq!(
        config.logging_config().file_config().filename_suffix(),
        "txt"
    );
    assert_eq!(config.logging_config().file_config().max_files(), 3);
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
  protocol: "mock"
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
  protocol: "mock"
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
    assert!(DEFAULT_ASSISTANT_SYSTEM_PROMPT.contains("#!openjarvis[image:"));
    assert!(DEFAULT_ASSISTANT_SYSTEM_PROMPT.contains("绝对路径"));
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
  protocol: "mock"
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
fn malformed_logging_config_with_blank_level_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-logging-blank-level");
    fixture.write_yaml(
        r#"
logging:
  level: "   "
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let error =
        AppConfig::from_path(fixture.config_path()).expect_err("blank logging level is invalid");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("logging.level must not be blank"));
}

#[test]
fn malformed_logging_config_without_any_sink_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-logging-without-sink");
    fixture.write_yaml(
        r#"
logging:
  stderr: false
  file:
    enabled: false
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let error = AppConfig::from_path(fixture.config_path())
        .expect_err("logging without any sink should be invalid");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("logging requires at least one enabled sink"));
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
  protocol: "mock"
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
  protocol: "mock"
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
  protocol: "mock"
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
  protocol: "mock"
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
  protocol: "mock"
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
  protocol: "mock"
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

#[test]
fn compact_and_budget_config_parse_from_yaml() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    auto_compact: true
    runtime_threshold_ratio: 0.9
    tool_visible_threshold_ratio: 0.75
    reserved_output_tokens: 512
    mock_compacted_assistant: "这是压缩后的上下文，使用 mock 固定摘要。"
llm:
  protocol: "mock"
  provider: "mock"
  context_window_tokens: 16384
  tokenizer: "chars_div4"
"#,
    )
    .expect("compact config should parse");

    assert!(config.agent_config().compact_config().enabled());
    assert!(config.agent_config().compact_config().auto_compact());
    assert_eq!(
        config
            .agent_config()
            .compact_config()
            .runtime_threshold_ratio(),
        0.9
    );
    assert_eq!(
        config
            .agent_config()
            .compact_config()
            .tool_visible_threshold_ratio(),
        0.75
    );
    assert_eq!(
        config
            .agent_config()
            .compact_config()
            .reserved_output_tokens(),
        512
    );
    assert_eq!(
        config
            .agent_config()
            .compact_config()
            .mock_compacted_assistant(),
        Some("这是压缩后的上下文，使用 mock 固定摘要。")
    );
    assert_eq!(config.llm_config().context_window_tokens(), 16384);
    assert_eq!(config.llm_config().tokenizer, "chars_div4");
}

#[test]
fn kimi_k2_5_token_limits_fall_back_to_official_defaults_when_omitted() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
llm:
  protocol: "openai"
  provider: "ark"
  model: "kimi-k2.5"
  base_url: "https://ark.cn-beijing.volces.com/api/coding/v3"
  tokenizer: "chars_div4"
"#,
    )
    .expect("kimi config should parse");

    assert_eq!(config.llm_config().context_window_tokens(), 262144);
    assert_eq!(config.llm_config().max_output_tokens(), 32768);
}

#[test]
fn malformed_compact_config_with_visible_threshold_above_runtime_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-compact-threshold-invalid");
    fixture.write_yaml(
        r#"
agent:
  compact:
    enabled: true
    auto_compact: true
    runtime_threshold_ratio: 0.7
    tool_visible_threshold_ratio: 0.8
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let error = AppConfig::from_path(fixture.config_path())
        .expect_err("invalid compact threshold ordering should be rejected");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("tool_visible_threshold_ratio"));
    assert!(error_chain.contains("runtime_threshold_ratio"));
}

#[test]
fn malformed_compact_config_with_blank_mock_summary_is_rejected() {
    let fixture = ConfigFixture::new("openjarvis-compact-mock-summary-invalid");
    fixture.write_yaml(
        r#"
agent:
  compact:
    enabled: true
    mock_compacted_assistant: "   "
llm:
  protocol: "mock"
  provider: "mock"
"#,
    );

    let error = AppConfig::from_path(fixture.config_path())
        .expect_err("blank compact mock summary should be rejected");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("mock_compacted_assistant"));
}
