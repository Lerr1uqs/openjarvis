//! Protocol tests for the demo-only internal MCP servers.

use super::super::{build_thread, call_tool, list_tools};
use super::{DemoHttpServerProcess, demo_http_config, demo_stdio_config, openjarvis_bin};
use openjarvis::agent::{McpServerState, McpTransport, ToolCallRequest, ToolRegistry, ToolSource};
use openjarvis::config::AppConfig;
use serde_json::json;
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct ExternalMcpConfigFixture {
    root: PathBuf,
    config_path: PathBuf,
}

impl ExternalMcpConfigFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let config_path = root.join("config.yaml");
        Self { root, config_path }
    }

    fn config_path(&self) -> &Path {
        &self.config_path
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

impl Drop for ExternalMcpConfigFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn demo_stdio_internal_subcommand_exposes_mcp_tools() {
    let config = demo_stdio_config(true);
    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should build");
    let mut thread_context = build_thread("thread_demo_stdio_internal");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].state, McpServerState::Healthy);
    assert_eq!(servers[0].transport, McpTransport::Stdio);
    assert_eq!(servers[0].tool_count, 3);
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

    let sum_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_stdio__sum".to_string(),
            arguments: json!({
                "a": 4,
                "b": 5,
            }),
        },
    )
    .await
    .expect("stdio sum tool should succeed");
    assert!(!sum_result.is_error);
    assert_eq!(sum_result.metadata["server_name"], "demo_stdio");
    assert_eq!(sum_result.metadata["structured_content"]["sum"], 9);
    assert_eq!(
        sum_result.metadata["structured_content"]["transport"],
        "stdio"
    );

    let health_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_stdio__health_probe".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("stdio health tool should succeed");
    assert!(!health_result.is_error);
    assert_eq!(health_result.metadata["structured_content"]["ok"], true);
    assert_eq!(
        health_result.metadata["structured_content"]["transport"],
        "stdio"
    );
}

#[tokio::test]
async fn demo_http_internal_subcommand_exposes_mcp_tools() {
    let demo_server = DemoHttpServerProcess::spawn()
        .await
        .expect("demo http server should start");
    let config = demo_http_config(demo_server.base_url(), true);
    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should build");
    let mut thread_context = build_thread("thread_demo_http_internal");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_http");
    assert_eq!(servers[0].state, McpServerState::Healthy);
    assert_eq!(servers[0].transport, McpTransport::StreamableHttp);
    assert_eq!(servers[0].endpoint, demo_server.base_url());
    assert_eq!(servers[0].tool_count, 3);
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo_http" }),
        },
    )
    .await
    .expect("demo http toolset should load");

    let definitions = list_tools(&registry, &thread_context)
        .await
        .expect("thread-scoped definitions should list loaded MCP tools");
    let echo_definition = definitions
        .iter()
        .find(|definition| definition.name == "mcp__demo_http__echo")
        .expect("demo http echo tool should be registered");
    assert!(matches!(
        &echo_definition.source,
        ToolSource::Mcp(source)
            if source.server_name == "demo_http"
                && source.remote_tool_name == "echo"
                && source.transport == McpTransport::StreamableHttp
    ));

    let echo_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_http__echo".to_string(),
            arguments: json!({ "text": "http-ready" }),
        },
    )
    .await
    .expect("http echo tool should succeed");
    assert_eq!(echo_result.content, "[demo:streamable_http] http-ready");
    assert!(!echo_result.is_error);

    let health_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_http__health_probe".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("http health tool should succeed");
    assert_eq!(health_result.metadata["structured_content"]["ok"], true);
    assert_eq!(
        health_result.metadata["structured_content"]["transport"],
        "streamable_http"
    );
}

#[tokio::test]
async fn demo_stdio_server_can_be_loaded_from_external_mcp_json() {
    let fixture = ExternalMcpConfigFixture::new("openjarvis-demo-stdio-json");
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "demo_stdio_file": {
                "enabled": true,
                "command": openjarvis_bin().display().to_string(),
                "args": ["internal-mcp", "demo-stdio"]
            }
        }
    }));

    let config =
        AppConfig::from_path(fixture.config_path()).expect("external mcp json should load");
    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should build from external mcp json");
    let mut thread_context = build_thread("thread_demo_stdio_external");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_stdio_file");
    assert_eq!(servers[0].state, McpServerState::Healthy);
    assert_eq!(servers[0].transport, McpTransport::Stdio);
    assert_eq!(servers[0].tool_count, 3);
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo_stdio_file" }),
        },
    )
    .await
    .expect("external stdio toolset should load");

    let echo_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_stdio_file__echo".to_string(),
            arguments: json!({ "text": "json-stdio-ready" }),
        },
    )
    .await
    .expect("external json stdio tool should succeed");
    assert_eq!(echo_result.content, "[demo:stdio] json-stdio-ready");
    assert!(!echo_result.is_error);
}

#[tokio::test]
async fn demo_http_server_can_be_loaded_from_external_mcp_json() {
    let demo_server = DemoHttpServerProcess::spawn()
        .await
        .expect("demo http server should start");
    let fixture = ExternalMcpConfigFixture::new("openjarvis-demo-http-json");
    fixture.write_mcp_json(json!({
        "mcpServers": {
            "demo_http_file": {
                "enabled": true,
                "transport": "http",
                "url": demo_server.base_url()
            }
        }
    }));

    let config =
        AppConfig::from_path(fixture.config_path()).expect("external mcp json should load");
    let registry = ToolRegistry::from_config(config.agent_config().tool_config())
        .await
        .expect("tool registry should build from external mcp json");
    let mut thread_context = build_thread("thread_demo_http_external");

    let servers = registry.mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_http_file");
    assert_eq!(servers[0].state, McpServerState::Healthy);
    assert_eq!(servers[0].transport, McpTransport::StreamableHttp);
    assert_eq!(servers[0].endpoint, demo_server.base_url());
    assert_eq!(servers[0].tool_count, 3);
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo_http_file" }),
        },
    )
    .await
    .expect("external http toolset should load");

    let echo_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "mcp__demo_http_file__echo".to_string(),
            arguments: json!({ "text": "json-http-ready" }),
        },
    )
    .await
    .expect("external json http tool should succeed");
    assert_eq!(
        echo_result.content,
        "[demo:streamable_http] json-http-ready"
    );
    assert!(!echo_result.is_error);
}
