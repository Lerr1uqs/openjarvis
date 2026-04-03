//! Demo-only MCP servers bundled with OpenJarvis for protocol verification and tests.
//! These entrypoints are intentionally simple and are not intended as production tools.

use crate::cli::InternalMcpCommand;
use anyhow::{Context, Result};
use axum::Router;
use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService, io::stdio,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use tracing::info;

const DEMO_HTTP_READY_PREFIX: &str = "OPENJARVIS_DEMO_HTTP_READY=";

/// Run one internal demo MCP subcommand.
pub async fn run_internal_demo_command(command: &InternalMcpCommand) -> Result<()> {
    match command {
        InternalMcpCommand::DemoStdio => run_demo_stdio_server().await,
        InternalMcpCommand::DemoHttp { bind } => run_demo_http_server(bind).await,
    }
}

/// Run the demo MCP server over stdio.
pub async fn run_demo_stdio_server() -> Result<()> {
    let server = DemoMcpServer::new("stdio");
    let running = server
        .serve(stdio())
        .await
        .context("failed to start demo stdio mcp server")?;
    let _ = running
        .waiting()
        .await
        .context("demo stdio mcp server task failed")?;
    Ok(())
}

/// Run the demo MCP server over Streamable HTTP on the provided bind address.
pub async fn run_demo_http_server(bind: &str) -> Result<()> {
    let service: StreamableHttpService<DemoMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(DemoMcpServer::new("streamable_http")),
            Default::default(),
            StreamableHttpServerConfig::default(),
        );
    let router = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind demo http mcp server to {bind}"))?;
    let base_url = format!("http://{}/mcp", listener.local_addr()?);

    announce_demo_http_server_ready(&base_url)?;
    info!(%base_url, "demo http mcp server ready");

    axum::serve(listener, router)
        .await
        .context("demo http mcp server exited unexpectedly")
}

fn announce_demo_http_server_ready(base_url: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{DEMO_HTTP_READY_PREFIX}{base_url}")
        .context("failed to write demo http mcp server ready line")?;
    stdout
        .flush()
        .context("failed to flush demo http mcp server ready line")?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    text: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct SumRequest {
    a: i64,
    b: i64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct SumResponse {
    transport: String,
    sum: i64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct HealthProbeResponse {
    ok: bool,
    transport: String,
}

#[derive(Debug, Clone)]
struct DemoMcpServer {
    tool_router: ToolRouter<Self>,
    transport_label: String,
}

impl DemoMcpServer {
    fn new(transport_label: impl Into<String>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            transport_label: transport_label.into(),
        }
    }
}

#[tool_router]
impl DemoMcpServer {
    #[tool(description = "Echo back the provided text for MCP demo verification.")]
    fn echo(&self, Parameters(EchoRequest { text }): Parameters<EchoRequest>) -> String {
        format!("[demo:{}] {text}", self.transport_label)
    }

    #[tool(description = "Add two integers and return a structured demo payload.")]
    fn sum(&self, Parameters(SumRequest { a, b }): Parameters<SumRequest>) -> Json<SumResponse> {
        Json(SumResponse {
            transport: self.transport_label.clone(),
            sum: a + b,
        })
    }

    #[tool(description = "Return a fixed health payload used by MCP startup probes and tests.")]
    fn health_probe(&self) -> Json<HealthProbeResponse> {
        Json(HealthProbeResponse {
            ok: true,
            transport: self.transport_label.clone(),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DemoMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "OpenJarvis demo-only MCP server used for protocol verification and tests.",
        )
    }
}
