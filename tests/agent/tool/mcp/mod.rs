//! Tests for the tool-managed MCP runtime and shared helpers for demo servers.

mod demo;

use super::{build_thread, call_tool, list_tools};
use anyhow::{Context, Result, bail};
use openjarvis::{
    agent::{McpServerState, ToolCallRequest, ToolRegistry, ToolSource},
    config::AppConfig,
};
use rmcp::{
    model::ClientInfo,
    serve_client,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::json;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
    time::{sleep, timeout},
};

const DEMO_HTTP_READY_PREFIX: &str = "OPENJARVIS_DEMO_HTTP_READY=";

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
  protocol: "mock"
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
  protocol: "mock"
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
        let mut child = Command::new(openjarvis_bin())
            .args(["internal-mcp", "demo-http", "--bind", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .context("demo http mcp server stdout should be piped")?;
        let base_url = wait_for_demo_http_server_ready(BufReader::new(stdout), &mut child).await?;

        Ok(Self { base_url, child })
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

async fn wait_for_demo_http_server_ready<T>(
    mut stdout: BufReader<T>,
    child: &mut Child,
) -> Result<String>
where
    T: tokio::io::AsyncRead + Unpin,
{
    let base_url = wait_for_demo_http_server_ready_line(&mut stdout, child).await?;

    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            let stderr = read_child_stderr(child).await;
            bail!(
                "demo http mcp server exited early with status {status}{}",
                format_child_stderr(&stderr)
            );
        }

        // TCP accept does not guarantee that the `/mcp` service has finished wiring up.
        // Probe the real MCP handshake so registry startup probes do not race the demo server.
        if timeout(
            Duration::from_millis(500),
            probe_demo_http_server(&base_url),
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            return Ok(base_url);
        }

        sleep(Duration::from_millis(100)).await;
    }

    bail!("timed out waiting for demo http mcp server at {base_url}")
}

async fn wait_for_demo_http_server_ready_line<T>(
    stdout: &mut BufReader<T>,
    child: &mut Child,
) -> Result<String>
where
    T: tokio::io::AsyncRead + Unpin,
{
    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            let stderr = read_child_stderr(child).await;
            bail!(
                "demo http mcp server exited before announcing readiness with status {status}{}",
                format_child_stderr(&stderr)
            );
        }

        let mut line = String::new();
        match timeout(Duration::from_millis(100), stdout.read_line(&mut line)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(_)) => {
                if let Some(base_url) = line.trim().strip_prefix(DEMO_HTTP_READY_PREFIX) {
                    return Ok(base_url.to_string());
                }
            }
            Ok(Err(error)) => {
                return Err(error).context("failed to read demo http mcp server ready line");
            }
            Err(_) => {}
        }
    }

    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(1), child.wait()).await;
    let stderr = read_child_stderr(child).await;
    bail!(
        "timed out waiting for demo http mcp server ready line{}",
        format_child_stderr(&stderr)
    )
}

async fn probe_demo_http_server(base_url: &str) -> Result<()> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(base_url.to_string()),
    );
    let mut client = serve_client(ClientInfo::default(), transport).await?;
    match client.peer().list_all_tools().await {
        Ok(_) => {
            let _ = client.close().await;
            Ok(())
        }
        Err(error) => {
            let _ = client.close().await;
            Err(error.into())
        }
    }
}

async fn read_child_stderr(child: &mut Child) -> String {
    let Some(stderr) = child.stderr.take() else {
        return String::new();
    };
    let mut stderr_reader = BufReader::new(stderr);
    let mut output = String::new();
    match timeout(
        Duration::from_secs(1),
        stderr_reader.read_to_string(&mut output),
    )
    .await
    {
        Ok(Ok(_)) => output.trim().to_string(),
        Ok(Err(error)) => format!("failed to read child stderr: {error}"),
        Err(_) => "timed out while reading child stderr".to_string(),
    }
}

fn format_child_stderr(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!("; stderr: {stderr}")
    }
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

    let toolsets = registry.list_toolsets().await;
    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].name, "demo_stdio");
    let mut thread_context = build_thread("thread_demo_stdio");
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo_stdio" }),
        },
    )
    .await
    .expect("demo stdio toolset should load");
    let definitions = list_tools(&registry, &thread_context)
        .await
        .expect("thread-scoped definitions should list loaded MCP tools");
    assert_eq!(definitions.len(), 5);
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

    let echo_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_stdio__echo".to_string(),
            arguments: json!({ "text": "ping" }),
        },
    )
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

    let error = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_stdio__echo".to_string(),
            arguments: json!({ "text": "ping" }),
        },
    )
    .await
    .expect_err("disabled MCP tool should be removed from registry");
    assert!(error.to_string().contains("disabled"));
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
  protocol: "mock"
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
