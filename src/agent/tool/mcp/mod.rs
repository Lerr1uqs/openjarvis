//! Tool-managed MCP runtime support, including server state, transport probing,
//! namespaced tool exposure, and demo-only MCP handlers for local verification.

pub mod demo;

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolInputSchema, ToolSource,
    ToolSourceMcp,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rmcp::{
    model::{CallToolRequestParams, ClientInfo, Content, ResourceContents, Tool as McpRemoteTool},
    serve_client,
    service::{RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::{process::Command, sync::RwLock};

type McpClient = RunningService<RoleClient, ClientInfo>;

/// Supported MCP transports managed by OpenJarvis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransport {
    /// Connect to an MCP server over stdio by spawning a child process.
    Stdio,
    /// Connect to an MCP server over Streamable HTTP.
    StreamableHttp,
}

impl McpTransport {
    /// Return the stable transport identifier used in config and status payloads.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::McpTransport;
    ///
    /// assert_eq!(McpTransport::Stdio.as_str(), "stdio");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable_http",
        }
    }
}

/// One MCP server definition loaded into the tool runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDefinition {
    /// Unique MCP server name inside the tool runtime.
    pub name: String,
    /// Transport used to reach the server.
    pub transport: McpTransport,
    /// Whether the server should be enabled during startup probing.
    pub enabled: bool,
    /// Command used for stdio servers.
    pub command: Option<String>,
    /// Command arguments used for stdio servers.
    pub args: Vec<String>,
    /// Environment variables injected into stdio servers.
    pub env: HashMap<String, String>,
    /// URL used for Streamable HTTP servers.
    pub url: Option<String>,
}

impl McpServerDefinition {
    /// Return a human-readable endpoint summary for diagnostics.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::{McpServerDefinition, McpTransport};
    /// use std::collections::HashMap;
    ///
    /// let definition = McpServerDefinition {
    ///     name: "demo".to_string(),
    ///     transport: McpTransport::Stdio,
    ///     enabled: true,
    ///     command: Some("openjarvis".to_string()),
    ///     args: vec!["internal-mcp".to_string()],
    ///     env: HashMap::new(),
    ///     url: None,
    /// };
    ///
    /// assert!(definition.endpoint().contains("openjarvis"));
    /// ```
    pub fn endpoint(&self) -> String {
        match self.transport {
            McpTransport::Stdio => {
                let command = self.command.as_deref().unwrap_or_default();
                if self.args.is_empty() {
                    command.to_string()
                } else {
                    format!("{command} {}", self.args.join(" "))
                }
            }
            McpTransport::StreamableHttp => self.url.clone().unwrap_or_default(),
        }
    }

    /// Validate that the definition is complete for its selected transport.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("mcp server name must not be blank");
        }

        match self.transport {
            McpTransport::Stdio => {
                let Some(command) = self.command.as_ref() else {
                    bail!("mcp server `{}` stdio command is required", self.name);
                };
                if command.trim().is_empty() {
                    bail!("mcp server `{}` stdio command must not be blank", self.name);
                }
            }
            McpTransport::StreamableHttp => {
                let Some(url) = self.url.as_ref() else {
                    bail!("mcp server `{}` streamable_http url is required", self.name);
                };
                if url.trim().is_empty() {
                    bail!(
                        "mcp server `{}` streamable_http url must not be blank",
                        self.name
                    );
                }
            }
        }

        Ok(())
    }
}

/// Current runtime state for one managed MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerState {
    /// The server is disabled and contributes no tools.
    Disabled,
    /// The server is enabled and its tool list has been probed successfully.
    Healthy,
    /// The server is enabled but probing failed.
    Unhealthy,
}

impl McpServerState {
    /// Return the stable state identifier used in status payloads.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::McpServerState;
    ///
    /// assert_eq!(McpServerState::Healthy.as_str(), "healthy");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// A query-friendly snapshot of one MCP server managed by the tool runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSnapshot {
    /// MCP server name.
    pub name: String,
    /// Selected transport.
    pub transport: McpTransport,
    /// Whether the server is currently enabled.
    pub enabled: bool,
    /// Current health state.
    pub state: McpServerState,
    /// Endpoint summary used for diagnostics.
    pub endpoint: String,
    /// Number of tools currently exported from this server.
    pub tool_count: usize,
    /// Last probe error, if any.
    pub last_error: Option<String>,
    /// Last successful or failed probe timestamp.
    pub last_checked_at: Option<DateTime<Utc>>,
}

/// A query-friendly snapshot of one MCP-backed tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSnapshot {
    /// Tool name exposed to the LLM and OpenJarvis tool registry.
    pub tool_name: String,
    /// Original tool name advertised by the remote MCP server.
    pub remote_tool_name: String,
    /// Owning MCP server name.
    pub server_name: String,
    /// Owning transport.
    pub transport: McpTransport,
    /// Tool description from the remote MCP server.
    pub description: String,
}

#[derive(Debug)]
struct ManagedMcpServer {
    definition: McpServerDefinition,
    enabled: bool,
    state: McpServerState,
    last_error: Option<String>,
    last_checked_at: Option<DateTime<Utc>>,
    discovered_tools: Vec<McpVisibleTool>,
    client: Option<McpClient>,
}

impl ManagedMcpServer {
    fn snapshot(&self) -> McpServerSnapshot {
        McpServerSnapshot {
            name: self.definition.name.clone(),
            transport: self.definition.transport,
            enabled: self.enabled,
            state: self.state,
            endpoint: self.definition.endpoint(),
            tool_count: self.discovered_tools.len(),
            last_error: self.last_error.clone(),
            last_checked_at: self.last_checked_at,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct McpVisibleTool {
    pub(crate) definition: ToolDefinition,
    pub(crate) remote_tool_name: String,
    pub(crate) server_name: String,
}

/// MCP server runtime owned by the tool subsystem.
#[derive(Default)]
pub(crate) struct McpManager {
    servers: RwLock<HashMap<String, ManagedMcpServer>>,
}

impl McpManager {
    /// Create an empty MCP manager.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register one MCP server definition without enabling it.
    pub(crate) async fn register(&self, definition: McpServerDefinition) -> Result<()> {
        definition.validate()?;

        let mut servers = self.servers.write().await;
        if servers.contains_key(&definition.name) {
            bail!("mcp server `{}` is already registered", definition.name);
        }

        servers.insert(
            definition.name.clone(),
            ManagedMcpServer {
                definition,
                enabled: false,
                state: McpServerState::Disabled,
                last_error: None,
                last_checked_at: None,
                discovered_tools: Vec::new(),
                client: None,
            },
        );
        Ok(())
    }

    /// Load a batch of MCP server definitions and startup-enable the configured ones.
    pub(crate) async fn load_definitions(
        &self,
        definitions: impl IntoIterator<Item = McpServerDefinition>,
    ) -> Result<()> {
        let mut startup_enabled = Vec::new();

        for definition in definitions {
            if definition.enabled {
                startup_enabled.push(definition.name.clone());
            }
            self.register(definition).await?;
        }

        for name in startup_enabled {
            let _ = self.enable_server(&name).await;
        }

        Ok(())
    }

    /// Return snapshots for all known MCP servers.
    pub(crate) async fn list_servers(&self) -> Vec<McpServerSnapshot> {
        let mut snapshots = self
            .servers
            .read()
            .await
            .values()
            .map(ManagedMcpServer::snapshot)
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.name.cmp(&right.name));
        snapshots
    }

    /// Return visible MCP-backed tool snapshots from healthy enabled servers.
    pub(crate) async fn list_tools(&self) -> Vec<McpToolSnapshot> {
        let mut tools = self
            .visible_tools()
            .await
            .into_iter()
            .map(|tool| McpToolSnapshot {
                tool_name: tool.definition.name.clone(),
                remote_tool_name: tool.remote_tool_name,
                server_name: tool.server_name,
                transport: tool
                    .definition
                    .source
                    .mcp_transport()
                    .unwrap_or(McpTransport::Stdio),
                description: tool.definition.description,
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
        tools
    }

    /// Enable a server, probe it, and expose its tools if the probe succeeds.
    pub(crate) async fn enable_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let definition = self.server_definition(name).await?;
        let checked_at = Utc::now();
        let probe_result = Self::probe_server(&definition).await;

        match probe_result {
            Ok((client, discovered_tools)) => {
                let replaced_client = {
                    let mut servers = self.servers.write().await;
                    let server = servers
                        .get_mut(name)
                        .with_context(|| format!("mcp server `{name}` is not registered"))?;
                    let replaced_client = server.client.take();
                    server.enabled = true;
                    server.state = McpServerState::Healthy;
                    server.last_error = None;
                    server.last_checked_at = Some(checked_at);
                    server.discovered_tools = discovered_tools;
                    server.client = Some(client);
                    replaced_client
                };
                close_client(replaced_client).await;
                Ok(self.server_snapshot(name).await?)
            }
            Err(error) => {
                let error_text = format!("{error:#}");
                let replaced_client = {
                    let mut servers = self.servers.write().await;
                    let server = servers
                        .get_mut(name)
                        .with_context(|| format!("mcp server `{name}` is not registered"))?;
                    let replaced_client = server.client.take();
                    server.enabled = true;
                    server.state = McpServerState::Unhealthy;
                    server.last_error = Some(error_text.clone());
                    server.last_checked_at = Some(checked_at);
                    server.discovered_tools.clear();
                    replaced_client
                };
                close_client(replaced_client).await;
                bail!(error_text)
            }
        }
    }

    /// Disable a server and remove its tools from the runtime.
    pub(crate) async fn disable_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let replaced_client = {
            let mut servers = self.servers.write().await;
            let server = servers
                .get_mut(name)
                .with_context(|| format!("mcp server `{name}` is not registered"))?;
            let replaced_client = server.client.take();
            server.enabled = false;
            server.state = McpServerState::Disabled;
            server.discovered_tools.clear();
            server.last_checked_at = Some(Utc::now());
            replaced_client
        };
        close_client(replaced_client).await;
        self.server_snapshot(name).await
    }

    /// Re-probe an enabled server and update its exported tool set.
    pub(crate) async fn refresh_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let enabled = {
            let servers = self.servers.read().await;
            let server = servers
                .get(name)
                .with_context(|| format!("mcp server `{name}` is not registered"))?;
            server.enabled
        };

        if !enabled {
            return self.server_snapshot(name).await;
        }

        self.enable_server(name).await
    }

    /// Return visible MCP tool definitions for ToolRegistry sync.
    pub(crate) async fn visible_tools(&self) -> Vec<McpVisibleTool> {
        let mut tools = self
            .servers
            .read()
            .await
            .values()
            .filter(|server| server.enabled && server.state == McpServerState::Healthy)
            .flat_map(|server| server.discovered_tools.clone())
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.definition.name.cmp(&right.definition.name));
        tools
    }

    /// Invoke one namespaced MCP tool through its owning remote server.
    pub(crate) async fn call_tool(
        &self,
        server_name: &str,
        remote_tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult> {
        let peer = {
            let servers = self.servers.read().await;
            let server = servers
                .get(server_name)
                .with_context(|| format!("mcp server `{server_name}` is not registered"))?;
            let Some(client) = server.client.as_ref() else {
                bail!("mcp server `{server_name}` is not connected");
            };
            client.peer().clone()
        };

        let arguments = match arguments {
            Value::Null => None,
            Value::Object(object) => Some(object),
            other => {
                bail!("mcp tool `{remote_tool_name}` arguments must be a JSON object, got {other}")
            }
        };

        let mut params = CallToolRequestParams::new(remote_tool_name.to_string());
        params.arguments = arguments;

        let result = peer
            .call_tool(params)
            .await
            .with_context(|| format!("failed to call mcp tool `{remote_tool_name}`"))?;

        Ok(normalize_call_tool_result(
            server_name,
            remote_tool_name,
            result,
        ))
    }

    async fn server_definition(&self, name: &str) -> Result<McpServerDefinition> {
        self.servers
            .read()
            .await
            .get(name)
            .map(|server| server.definition.clone())
            .with_context(|| format!("mcp server `{name}` is not registered"))
    }

    async fn server_snapshot(&self, name: &str) -> Result<McpServerSnapshot> {
        self.servers
            .read()
            .await
            .get(name)
            .map(ManagedMcpServer::snapshot)
            .with_context(|| format!("mcp server `{name}` is not registered"))
    }

    async fn probe_server(
        definition: &McpServerDefinition,
    ) -> Result<(McpClient, Vec<McpVisibleTool>)> {
        let mut client = connect_server(definition)
            .await
            .with_context(|| format!("failed to connect mcp server `{}`", definition.name))?;

        let remote_tools = match client.peer().list_all_tools().await {
            Ok(remote_tools) => remote_tools,
            Err(error) => {
                let _ = client.close().await;
                return Err(anyhow::anyhow!(error))
                    .with_context(|| format!("failed to list tools from `{}`", definition.name));
            }
        };

        let visible_tools = map_remote_tools(definition, remote_tools);
        Ok((client, visible_tools))
    }
}

#[derive(Clone)]
pub(crate) struct McpToolHandler {
    definition: ToolDefinition,
    manager: Arc<McpManager>,
    server_name: String,
    remote_tool_name: String,
}

impl McpToolHandler {
    /// Create a new MCP-backed tool handler.
    pub(crate) fn new(manager: Arc<McpManager>, visible_tool: McpVisibleTool) -> Self {
        Self {
            definition: visible_tool.definition,
            manager,
            server_name: visible_tool.server_name,
            remote_tool_name: visible_tool.remote_tool_name,
        }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        self.manager
            .call_tool(&self.server_name, &self.remote_tool_name, request.arguments)
            .await
    }
}

async fn connect_server(definition: &McpServerDefinition) -> Result<McpClient> {
    match definition.transport {
        McpTransport::Stdio => {
            let command = definition.command.as_ref().with_context(|| {
                format!("mcp server `{}` missing stdio command", definition.name)
            })?;
            let mut process = Command::new(command);
            process.args(&definition.args);
            for (key, value) in &definition.env {
                process.env(key, value);
            }

            let transport = TokioChildProcess::new(process).with_context(|| {
                format!("failed to spawn stdio mcp server `{}`", definition.name)
            })?;
            serve_client(ClientInfo::default(), transport)
                .await
                .with_context(|| {
                    format!(
                        "failed to initialize stdio mcp server `{}`",
                        definition.name
                    )
                })
        }
        McpTransport::StreamableHttp => {
            let url = definition.url.as_ref().with_context(|| {
                format!(
                    "mcp server `{}` missing streamable_http url",
                    definition.name
                )
            })?;
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.clone()),
            );
            serve_client(ClientInfo::default(), transport)
                .await
                .with_context(|| {
                    format!(
                        "failed to initialize streamable_http mcp server `{}`",
                        definition.name
                    )
                })
        }
    }
}

async fn close_client(mut client: Option<McpClient>) {
    if let Some(client) = client.as_mut() {
        let _ = client.close().await;
    }
}

fn map_remote_tools(
    definition: &McpServerDefinition,
    remote_tools: Vec<McpRemoteTool>,
) -> Vec<McpVisibleTool> {
    let mut seen_names = HashMap::<String, usize>::new();
    let mut visible_tools = Vec::with_capacity(remote_tools.len());

    for remote_tool in remote_tools {
        let base_name = format!(
            "mcp__{}__{}",
            normalize_tool_segment(&definition.name),
            normalize_tool_segment(&remote_tool.name)
        );
        let collision_count = seen_names.entry(base_name.clone()).or_insert(0);
        *collision_count += 1;
        let exposed_name = if *collision_count == 1 {
            base_name
        } else {
            format!("{base_name}_{}", collision_count)
        };

        let description = remote_tool
            .description
            .as_deref()
            .unwrap_or("Remote MCP tool")
            .to_string();
        let input_schema =
            ToolInputSchema::new(Value::Object(remote_tool.input_schema.as_ref().clone()));
        let remote_tool_name = remote_tool.name.to_string();

        visible_tools.push(McpVisibleTool {
            definition: ToolDefinition {
                name: exposed_name,
                description,
                input_schema,
                source: ToolSource::Mcp(ToolSourceMcp {
                    server_name: definition.name.clone(),
                    remote_tool_name: remote_tool_name.clone(),
                    transport: definition.transport,
                }),
            },
            remote_tool_name,
            server_name: definition.name.clone(),
        });
    }

    visible_tools
}

fn normalize_tool_segment(segment: &str) -> String {
    let normalized = segment
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => character,
            _ => '_',
        })
        .collect::<String>();

    if normalized.is_empty() {
        "tool".to_string()
    } else {
        normalized
    }
}

fn normalize_call_tool_result(
    server_name: &str,
    remote_tool_name: &str,
    result: rmcp::model::CallToolResult,
) -> ToolCallResult {
    let rendered_content = render_mcp_content(&result.content, result.structured_content.as_ref());
    let is_error = result.is_error.unwrap_or(false);

    ToolCallResult {
        content: rendered_content,
        metadata: json!({
            "source": "mcp",
            "server_name": server_name,
            "remote_tool_name": remote_tool_name,
            "structured_content": result.structured_content,
            "content_block_count": result.content.len(),
        }),
        is_error,
    }
}

fn render_mcp_content(contents: &[Content], structured_content: Option<&Value>) -> String {
    let rendered = contents
        .iter()
        .map(render_content_block)
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>();

    if !rendered.is_empty() {
        return rendered.join("\n");
    }

    structured_content
        .map(Value::to_string)
        .unwrap_or_else(|| "{}".to_string())
}

fn render_content_block(content: &Content) -> String {
    match content.raw.as_text() {
        Some(text) => text.text.clone(),
        None => match content.raw.as_resource() {
            Some(resource) => match &resource.resource {
                ResourceContents::TextResourceContents { text, .. } => text.clone(),
                ResourceContents::BlobResourceContents { .. } => String::new(),
            },
            None => serde_json::to_string(content).unwrap_or_else(|_| String::new()),
        },
    }
}
