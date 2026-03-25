//! Shared tool traits, schemas, and registry for the built-in agent tool set.

pub mod edit;
pub mod mcp;
pub mod read;
pub mod shell;
pub mod skill;
pub mod write;

use crate::config::{AgentMcpServerConfig, AgentMcpServerTransportConfig, AgentToolConfig};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;

pub use edit::EditTool;
pub use mcp::{
    McpServerDefinition, McpServerSnapshot, McpServerState, McpToolSnapshot, McpTransport,
};
pub use read::ReadTool;
pub use shell::ShellTool;
pub use skill::{LoadSkillTool, LoadedSkill, LoadedSkillFile, SkillManifest, SkillRegistry};
pub use write::WriteTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
    pub source: ToolSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSource {
    Builtin,
    Mcp(ToolSourceMcp),
}

impl ToolSource {
    /// Return the MCP transport when the tool comes from an MCP server.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ToolSource;
    ///
    /// assert!(ToolSource::Builtin.mcp_transport().is_none());
    /// ```
    pub fn mcp_transport(&self) -> Option<McpTransport> {
        match self {
            ToolSource::Builtin => None,
            ToolSource::Mcp(source) => Some(source.transport),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSourceMcp {
    pub server_name: String,
    pub remote_tool_name: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSchemaProtocol {
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInputSchema {
    json_schema: Value,
}

impl ToolInputSchema {
    /// Wrap a protocol-agnostic JSON Schema definition.
    pub fn new(json_schema: Value) -> Self {
        Self { json_schema }
    }

    /// Return the stored protocol-agnostic JSON Schema.
    pub fn json_schema(&self) -> &Value {
        &self.json_schema
    }

    /// Project the stored schema into the OpenAI tool schema shape.
    pub fn for_openai(&self) -> Value {
        self.json_schema.clone()
    }

    /// Project the stored schema into the Anthropic tool schema shape.
    pub fn for_anthropic(&self) -> Value {
        self.json_schema.clone()
    }

    /// Project the stored schema for the selected LLM protocol.
    pub fn for_protocol(&self, protocol: ToolSchemaProtocol) -> Value {
        match protocol {
            ToolSchemaProtocol::OpenAi => self.for_openai(),
            ToolSchemaProtocol::Anthropic => self.for_anthropic(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub content: String,
    pub metadata: Value,
    pub is_error: bool,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the definition exposed to the agent loop and the model provider.
    fn definition(&self) -> ToolDefinition;

    /// Execute one tool call and return a normalized result payload.
    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult>;
}

pub struct ToolRegistry {
    // AGENT-TODO: 对String起别名
    handlers: RwLock<HashMap<String, Arc<dyn ToolHandler>>>,
    mcp: Arc<mcp::McpManager>,
    skills: Arc<skill::SkillRegistry>,
}

pub fn tool_definition_from_args<T>(
    name: impl Into<String>,
    description: impl Into<String>,
) -> ToolDefinition
where
    T: JsonSchema,
{
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: tool_input_schema::<T>(),
        source: ToolSource::Builtin,
    }
}

pub fn parse_tool_arguments<T>(request: ToolCallRequest, tool_name: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(request.arguments)
        .with_context(|| format!("invalid `{tool_name}` tool arguments"))
}

impl ToolRegistry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            mcp: Arc::new(mcp::McpManager::new()),
            skills: Arc::new(skill::SkillRegistry::new()),
        }
    }

    /// Create an empty tool registry with explicit local skill roots.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::ToolRegistry;
    /// use std::path::PathBuf;
    ///
    /// let registry = ToolRegistry::with_skill_roots(vec![PathBuf::from(".skills")]);
    /// assert!(registry.list().await.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_skill_roots(skill_roots: Vec<PathBuf>) -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            mcp: Arc::new(mcp::McpManager::new()),
            skills: Arc::new(skill::SkillRegistry::with_roots(skill_roots)),
        }
    }

    /// Create a tool registry from the loaded `agent.tool` config section.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::ToolRegistry, config::AppConfig};
    ///
    /// let config = AppConfig::default();
    /// let registry = ToolRegistry::from_config(config.agent_config().tool_config()).await?;
    /// assert!(registry.mcp().list_servers().await.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &AgentToolConfig) -> Result<Self> {
        Self::from_config_with_skill_roots(config, vec![PathBuf::from(".skills")]).await
    }

    /// Create a tool registry from config with explicit local skill roots.
    ///
    /// This exists mainly so tests can opt into deterministic roots instead of using the
    /// workspace `.skills` directory.
    pub async fn from_config_with_skill_roots(
        config: &AgentToolConfig,
        skill_roots: Vec<PathBuf>,
    ) -> Result<Self> {
        let registry = Self::with_skill_roots(skill_roots);
        let definitions = build_mcp_server_definitions(config)?;
        registry.mcp.load_definitions(definitions).await?;
        registry.sync_mcp_handlers().await;
        Ok(registry)
    }

    /// Register one tool handler by its unique tool name.
    pub async fn register(&self, handler: Arc<dyn ToolHandler>) -> Result<()> {
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        if handlers.contains_key(&definition.name) {
            bail!("tool `{}` is already registered", definition.name);
        }

        handlers.insert(definition.name, handler);
        Ok(())
    }

    pub async fn register_builtin_tools(&self) -> Result<()> {
        // Register the current built-in four-tool set.
        self.register_if_missing(Arc::new(ReadTool::new())).await;
        self.register_if_missing(Arc::new(WriteTool::new())).await;
        self.register_if_missing(Arc::new(EditTool::new())).await;
        self.register_if_missing(Arc::new(ShellTool::new())).await;
        self.skills.reload().await?;
        self.sync_skill_handlers().await;
        Ok(())
    }

    /// Look up a registered tool and execute the request.
    pub async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let handler = self.handlers.read().await.get(&request.name).cloned();

        let Some(handler) = handler else {
            bail!("tool `{}` is not registered", request.name);
        };

        handler.call(request).await
    }

    /// Return all registered tool definitions.
    pub async fn list(&self) -> Vec<ToolDefinition> {
        let mut definitions = self
            .handlers
            .read()
            .await
            .values()
            .map(|handler| handler.definition())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    /// Return the MCP management API exposed by this tool registry.
    pub fn mcp(&self) -> ToolRegistryMcpApi<'_> {
        ToolRegistryMcpApi { registry: self }
    }

    /// Return the local skill management API exposed by this tool registry.
    pub fn skills(&self) -> ToolRegistrySkillApi<'_> {
        ToolRegistrySkillApi { registry: self }
    }

    async fn register_if_missing(&self, handler: Arc<dyn ToolHandler>) {
        // Register the handler only when the name is not present yet.
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        handlers.entry(definition.name).or_insert(handler);
    }

    async fn sync_mcp_handlers(&self) {
        let visible_tools = self.mcp.visible_tools().await;
        let mut handlers = self.handlers.write().await;
        handlers.retain(|_, handler| !matches!(handler.definition().source, ToolSource::Mcp(_)));

        for visible_tool in visible_tools {
            handlers.insert(
                visible_tool.definition.name.clone(),
                Arc::new(mcp::McpToolHandler::new(
                    Arc::clone(&self.mcp),
                    visible_tool,
                )),
            );
        }
    }

    async fn sync_skill_handlers(&self) {
        let mut handlers = self.handlers.write().await;
        if self.skills.has_enabled_skills().await {
            handlers.insert(
                "load_skill".to_string(),
                Arc::new(skill::LoadSkillTool::new(Arc::clone(&self.skills))),
            );
        } else {
            handlers.remove("load_skill");
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ToolRegistryMcpApi<'a> {
    registry: &'a ToolRegistry,
}

impl<'a> ToolRegistryMcpApi<'a> {
    /// List all managed MCP servers.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new();
    /// assert!(registry.mcp().list_servers().await.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_servers(&self) -> Vec<McpServerSnapshot> {
        self.registry.mcp.list_servers().await
    }

    /// List all currently visible MCP tools.
    pub async fn list_tools(&self) -> Vec<McpToolSnapshot> {
        self.registry.mcp.list_tools().await
    }

    /// Enable one managed MCP server and sync its tools into the registry.
    pub async fn enable_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let snapshot = self.registry.mcp.enable_server(name).await?;
        self.registry.sync_mcp_handlers().await;
        Ok(snapshot)
    }

    /// Disable one managed MCP server and remove its tools from the registry.
    pub async fn disable_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let snapshot = self.registry.mcp.disable_server(name).await?;
        self.registry.sync_mcp_handlers().await;
        Ok(snapshot)
    }

    /// Refresh one managed MCP server and sync any tool changes.
    pub async fn refresh_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let snapshot = self.registry.mcp.refresh_server(name).await?;
        self.registry.sync_mcp_handlers().await;
        Ok(snapshot)
    }
}

pub struct ToolRegistrySkillApi<'a> {
    registry: &'a ToolRegistry,
}

impl<'a> ToolRegistrySkillApi<'a> {
    /// Reload local skills from disk and sync the `load_skill` tool exposure.
    pub async fn reload(&self) -> Result<Vec<SkillManifest>> {
        let manifests = self.registry.skills.reload().await?;
        self.registry.sync_skill_handlers().await;
        Ok(manifests)
    }

    /// List all discovered local skills, including disabled entries.
    pub async fn list(&self) -> Vec<SkillManifest> {
        self.registry.skills.list().await
    }

    /// List all enabled local skills.
    pub async fn list_enabled(&self) -> Vec<SkillManifest> {
        self.registry.skills.list_enabled().await
    }

    /// Disable one local skill in memory and sync the `load_skill` tool exposure.
    pub async fn disable(&self, name: &str) -> Result<SkillManifest> {
        let manifest = self.registry.skills.disable(name).await?;
        self.registry.sync_skill_handlers().await;
        Ok(manifest)
    }

    /// Enable one local skill in memory and sync the `load_skill` tool exposure.
    pub async fn enable(&self, name: &str) -> Result<SkillManifest> {
        let manifest = self.registry.skills.enable(name).await?;
        self.registry.sync_skill_handlers().await;
        Ok(manifest)
    }

    /// Build the catalog prompt injected into the agent loop when skills are available.
    pub async fn catalog_prompt(&self) -> Option<String> {
        self.registry.skills.catalog_prompt().await
    }

    /// Enable only the selected local skills and disable every other discovered skill.
    pub async fn restrict_to(&self, names: &[String]) -> Result<Vec<SkillManifest>> {
        let manifests = self.registry.skills.restrict_to(names).await?;
        self.registry.sync_skill_handlers().await;
        Ok(manifests)
    }

    /// Load one enabled local skill by exact name.
    pub async fn load(&self, name: &str) -> Result<LoadedSkill> {
        self.registry.skills.load(name).await
    }
}

/// Return an empty object schema for tools that currently do not accept any arguments.
pub fn empty_tool_input_schema() -> ToolInputSchema {
    ToolInputSchema::new(json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false,
    }))
}

pub fn tool_input_schema<T>() -> ToolInputSchema
where
    T: JsonSchema,
{
    let mut schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("tool input schema should serialize");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    ToolInputSchema::new(schema)
}

fn build_mcp_server_definitions(config: &AgentToolConfig) -> Result<Vec<McpServerDefinition>> {
    let mut definitions = config
        .mcp_config()
        .servers()
        .iter()
        .map(|(name, server)| build_mcp_server_definition(name, server))
        .collect::<Result<Vec<_>>>()?;
    definitions.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(definitions)
}

fn build_mcp_server_definition(
    name: &str,
    server: &AgentMcpServerConfig,
) -> Result<McpServerDefinition> {
    match server.transport_config() {
        AgentMcpServerTransportConfig::Stdio { command, args, env } => Ok(McpServerDefinition {
            name: name.to_string(),
            transport: McpTransport::Stdio,
            enabled: server.enabled,
            command: Some(command.clone()),
            args: args.clone(),
            env: env.clone(),
            url: None,
        }),
        AgentMcpServerTransportConfig::StreamableHttp { url } => Ok(McpServerDefinition {
            name: name.to_string(),
            transport: McpTransport::StreamableHttp,
            enabled: server.enabled,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url.clone()),
        }),
    }
}
