//! Tests for the tool-managed MCP runtime and shared helpers for demo servers.

mod demo;

use anyhow::{Result, bail};
use openjarvis::{
    agent::{McpServerState, ToolCallRequest, ToolRegistry, ToolSource},
    config::AppConfig,
};
use serde_json::json;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    time::sleep,
};

pub(crate) fn openjarvis_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_openjarvis"))
}

pub(crate) fn demo_stdio_config(enabled: bool) -> AppConfig {
    serde_yaml::from_str(&format!(
        r#"
agent:
  tool:
    mcp:
      servers:
        demo_stdio:
          enabled: {enabled}
          transport: stdio
          command: '{command}'
          args:
            - internal-mcp
            - demo-stdio
llm:
  provider: "mock"
"#,
        command = openjarvis_bin().display(),
    ))
    .expect("demo stdio config should parse")
}

pub(crate) fn demo_http_config(url: &str, enabled: bool) -> AppConfig {
    serde_yaml::from_str(&format!(
        r#"
agent:
  tool:
    mcp:
      servers:
        demo_http:
          enabled: {enabled}
          transport: streamable_http
          url: "{url}"
llm:
  provider: "mock"
"#
    ))
    .expect("demo http config should parse")
}

pub(crate) struct DemoHttpServerProcess {
    base_url: String,
    child: Child,
}

impl DemoHttpServerProcess {
    pub(crate) async fn spawn() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        drop(listener);

        let mut child = Command::new(openjarvis_bin())
            .args(["internal-mcp", "demo-http", "--bind", &addr.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        wait_for_tcp_server(&addr.to_string(), &mut child).await?;

        Ok(Self {
            base_url: format!("http://{addr}/mcp"),
            child,
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for DemoHttpServerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn wait_for_tcp_server(addr: &str, child: &mut Child) -> Result<()> {
    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            bail!("demo http mcp server exited early with status {status}");
        }

        if TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }

        sleep(Duration::from_millis(100)).await;
    }

    bail!("timed out waiting for demo http mcp server at {addr}")
}

#[tokio::test]
async fn tool_registry_manages_stdio_mcp_server_lifecycle() {
    let config = demo_stdio_config(false);
    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should build");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_stdio");
    assert_eq!(servers[0].state, McpServerState::Disabled);
    assert_eq!(servers[0].tool_count, 0);

    let enabled_snapshot = registry
        .mcp()
        .enable_server("demo_stdio")
        .await
        .expect("demo stdio server should enable");
    assert_eq!(enabled_snapshot.state, McpServerState::Healthy);
    assert_eq!(enabled_snapshot.tool_count, 3);
    assert!(enabled_snapshot.last_checked_at.is_some());

    let mcp_tools = registry.mcp().list_tools().await;
    let tool_names = mcp_tools
        .iter()
        .map(|tool| tool.tool_name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec![
            "mcp__demo_stdio__echo",
            "mcp__demo_stdio__health_probe",
            "mcp__demo_stdio__sum",
        ]
    );

    let definitions = registry.list().await;
    assert_eq!(definitions.len(), 3);
    let echo_definition = definitions
        .iter()
        .find(|definition| definition.name == "mcp__demo_stdio__echo")
        .expect("echo tool should be registered");
    assert!(matches!(
        &echo_definition.source,
        ToolSource::Mcp(source)
            if source.server_name == "demo_stdio"
                && source.remote_tool_name == "echo"
    ));

    let echo_result = registry
        .call(ToolCallRequest {
            name: "mcp__demo_stdio__echo".to_string(),
            arguments: json!({ "text": "ping" }),
        })
        .await
        .expect("echo tool should succeed");
    assert_eq!(echo_result.content, "[demo:stdio] ping");
    assert_eq!(echo_result.metadata["server_name"], "demo_stdio");
    assert_eq!(echo_result.metadata["remote_tool_name"], "echo");

    let refreshed_snapshot = registry
        .mcp()
        .refresh_server("demo_stdio")
        .await
        .expect("enabled demo stdio server should refresh");
    assert_eq!(refreshed_snapshot.state, McpServerState::Healthy);
    assert_eq!(refreshed_snapshot.tool_count, 3);

    let disabled_snapshot = registry
        .mcp()
        .disable_server("demo_stdio")
        .await
        .expect("demo stdio server should disable");
    assert_eq!(disabled_snapshot.state, McpServerState::Disabled);
    assert_eq!(disabled_snapshot.tool_count, 0);
    assert!(registry.list().await.is_empty());

    let refreshed_disabled_snapshot = registry
        .mcp()
        .refresh_server("demo_stdio")
        .await
        .expect("disabled server refresh should return snapshot");
    assert_eq!(refreshed_disabled_snapshot.state, McpServerState::Disabled);

    let error = registry
        .call(ToolCallRequest {
            name: "mcp__demo_stdio__echo".to_string(),
            arguments: json!({ "text": "ping" }),
        })
        .await
        .expect_err("disabled MCP tool should be removed from registry");
    assert!(error.to_string().contains("not registered"));
}

#[tokio::test]
async fn tool_registry_marks_failed_startup_probe_as_unhealthy() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  tool:
    mcp:
      servers:
        broken_stdio:
          enabled: true
          transport: stdio
          command: "openjarvis-missing-demo-mcp-command"
llm:
  provider: "mock"
"#,
    )
    .expect("broken mcp config should parse");

    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should still build with unhealthy MCP");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "broken_stdio");
    assert!(servers[0].enabled);
    assert_eq!(servers[0].state, McpServerState::Unhealthy);
    assert_eq!(servers[0].tool_count, 0);
    assert!(
        servers[0]
            .last_error
            .as_deref()
            .expect("unhealthy startup should record error")
            .contains("failed to connect mcp server `broken_stdio`")
    );
    assert!(registry.mcp().list_tools().await.is_empty());
    assert!(registry.list().await.is_empty());
}
