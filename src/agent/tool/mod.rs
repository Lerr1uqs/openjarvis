//! Shared tool traits, schemas, and registry for the built-in agent tool set.

pub mod browser;
pub mod edit;
pub mod mcp;
pub mod read;
pub mod shell;
pub mod skill;
pub mod toolset;
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
pub use toolset::{ThreadToolRuntimeManager, ThreadToolRuntimeSnapshot, ToolsetCatalogEntry};
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

/// Thread-scoped runtime context attached to one tool invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallContext {
    pub thread_id: Option<String>,
}

impl ToolCallContext {
    /// Create a tool call context bound to one internal thread id.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ToolCallContext;
    ///
    /// let context = ToolCallContext::for_thread("thread-1");
    /// assert_eq!(context.thread_id(), Some("thread-1"));
    /// ```
    pub fn for_thread(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: Some(thread_id.into()),
        }
    }

    /// Return the bound internal thread id when present.
    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the definition exposed to the agent loop and the model provider.
    fn definition(&self) -> ToolDefinition;

    /// Execute one tool call and return a normalized result payload.
    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult>;

    /// Execute one tool call with optional thread-scoped runtime context.
    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let _ = context;
        self.call(request).await
    }
}

#[async_trait]
pub trait ToolsetRuntime: Send + Sync {
    /// Run cleanup when one toolset is unloaded from the current internal thread.
    async fn on_unload(&self, thread_id: &str) -> Result<()> {
        let _ = thread_id;
        Ok(())
    }
}

#[derive(Clone)]
struct StaticToolsetDefinition {
    entry: ToolsetCatalogEntry,
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    runtime: Option<Arc<dyn ToolsetRuntime>>,
}

#[derive(Clone)]
enum RegisteredToolset {
    Static(StaticToolsetDefinition),
    McpServer {
        entry: ToolsetCatalogEntry,
        server_name: String,
    },
}

pub struct ToolRegistry {
    // AGENT-TODO: 对String起别名
    always_visible_handlers: RwLock<HashMap<String, Arc<dyn ToolHandler>>>,
    toolsets: RwLock<HashMap<String, RegisteredToolset>>,
    thread_runtimes: Arc<toolset::ThreadToolRuntimeManager>,
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

impl RegisteredToolset {
    fn entry(&self) -> ToolsetCatalogEntry {
        match self {
            Self::Static(definition) => definition.entry.clone(),
            Self::McpServer { entry, .. } => entry.clone(),
        }
    }
}

impl ToolRegistry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self {
            always_visible_handlers: RwLock::new(HashMap::new()),
            toolsets: RwLock::new(HashMap::new()),
            thread_runtimes: Arc::new(toolset::ThreadToolRuntimeManager::new()),
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
            always_visible_handlers: RwLock::new(HashMap::new()),
            toolsets: RwLock::new(HashMap::new()),
            thread_runtimes: Arc::new(toolset::ThreadToolRuntimeManager::new()),
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
        registry.sync_mcp_toolsets().await?;
        Ok(registry)
    }

    /// Register one tool handler by its unique tool name.
    pub async fn register(&self, handler: Arc<dyn ToolHandler>) -> Result<()> {
        let definition = handler.definition();
        self.ensure_tool_name_available(&definition.name).await?;
        let mut handlers = self.always_visible_handlers.write().await;
        if handlers.contains_key(&definition.name) {
            bail!("tool `{}` is already registered", definition.name);
        }

        handlers.insert(definition.name, handler);
        Ok(())
    }

    /// Register one program-defined static toolset backed by concrete tool handlers.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::{ReadTool, ToolRegistry, ToolsetCatalogEntry};
    /// use std::sync::Arc;
    ///
    /// let registry = ToolRegistry::new();
    /// registry
    ///     .register_toolset(
    ///         ToolsetCatalogEntry::new("files", "Extra file helpers"),
    ///         vec![Arc::new(ReadTool::new())],
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn register_toolset(
        &self,
        entry: ToolsetCatalogEntry,
        handlers: Vec<Arc<dyn ToolHandler>>,
    ) -> Result<()> {
        self.register_toolset_with_runtime(entry, handlers, None)
            .await
    }

    /// Register one program-defined static toolset with optional lifecycle callbacks.
    pub async fn register_toolset_with_runtime(
        &self,
        entry: ToolsetCatalogEntry,
        handlers: Vec<Arc<dyn ToolHandler>>,
        runtime: Option<Arc<dyn ToolsetRuntime>>,
    ) -> Result<()> {
        validate_toolset_name(&entry.name)?;

        let mut routed_handlers = HashMap::new();
        for handler in handlers {
            let definition = handler.definition();
            if routed_handlers.contains_key(&definition.name) {
                bail!(
                    "toolset `{}` contains duplicate tool `{}`",
                    entry.name,
                    definition.name
                );
            }
            self.ensure_tool_name_available(&definition.name).await?;
            routed_handlers.insert(definition.name, handler);
        }

        let mut toolsets = self.toolsets.write().await;
        if toolsets.contains_key(&entry.name) {
            bail!("toolset `{}` is already registered", entry.name);
        }
        toolsets.insert(
            entry.name.clone(),
            RegisteredToolset::Static(StaticToolsetDefinition {
                entry,
                handlers: routed_handlers,
                runtime,
            }),
        );
        Ok(())
    }

    pub async fn register_builtin_tools(&self) -> Result<()> {
        // Register the current built-in four-tool set.
        self.register_if_missing(Arc::new(ReadTool::new())).await;
        self.register_if_missing(Arc::new(WriteTool::new())).await;
        self.register_if_missing(Arc::new(EditTool::new())).await;
        self.register_if_missing(Arc::new(ShellTool::new())).await;
        if !self.toolset_registered("browser").await {
            browser::register_browser_toolset(self).await?;
        }
        self.skills.reload().await?;
        self.sync_skill_handlers().await;
        Ok(())
    }

    /// Look up a registered tool and execute the request.
    pub async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let handler = self
            .always_visible_handlers
            .read()
            .await
            .get(&request.name)
            .cloned();

        let Some(handler) = handler else {
            bail!("tool `{}` is not registered", request.name);
        };

        handler
            .call_with_context(ToolCallContext::default(), request)
            .await
    }

    /// Return all registered tool definitions.
    pub async fn list(&self) -> Vec<ToolDefinition> {
        let mut definitions = self
            .always_visible_handlers
            .read()
            .await
            .values()
            .map(|handler| handler.definition())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    /// Return the thread-scoped visible tool definitions for one internal thread id.
    pub async fn list_for_thread(&self, thread_id: &str) -> Result<Vec<ToolDefinition>> {
        let mut definitions = self.list().await;
        definitions.push(load_toolset_definition());
        definitions.push(unload_toolset_definition());

        let loaded_toolsets = self.thread_runtimes.loaded_toolsets(thread_id).await;
        for toolset_name in loaded_toolsets {
            definitions.extend(
                self.resolve_toolset_handlers(&toolset_name)
                    .await?
                    .into_values()
                    .map(|handler| handler.definition()),
            );
        }

        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(definitions)
    }

    /// Execute one tool request within the current internal thread runtime.
    pub async fn call_for_thread(
        &self,
        thread_id: &str,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let context = ToolCallContext::for_thread(thread_id);
        match request.name.as_str() {
            "load_toolset" => self.load_toolset(thread_id, request).await,
            "unload_toolset" => self.unload_toolset(thread_id, request).await,
            _ => {
                if let Some(handler) = self
                    .always_visible_handlers
                    .read()
                    .await
                    .get(&request.name)
                    .cloned()
                {
                    return handler.call_with_context(context.clone(), request).await;
                }

                let loaded_toolsets = self.thread_runtimes.loaded_toolsets(thread_id).await;
                for toolset_name in loaded_toolsets {
                    let handlers = self.resolve_toolset_handlers(&toolset_name).await?;
                    if let Some(handler) = handlers.get(&request.name).cloned() {
                        return handler.call_with_context(context.clone(), request).await;
                    }
                }

                bail!(
                    "tool `{}` is not registered for thread `{}`",
                    request.name,
                    thread_id
                )
            }
        }
    }

    /// Return the compact toolset catalog prompt for one internal thread.
    pub async fn catalog_prompt(&self, thread_id: &str) -> Option<String> {
        let entries = self.list_toolsets().await;
        if entries.is_empty() {
            return None;
        }

        let loaded_toolsets = self.thread_runtimes.loaded_toolsets(thread_id).await;
        let loaded_summary = if loaded_toolsets.is_empty() {
            "none".to_string()
        } else {
            loaded_toolsets.join(", ")
        };
        let catalog = entries
            .into_iter()
            .map(|entry| format!("- {}: {}", entry.name, entry.description))
            .collect::<Vec<_>>()
            .join("\n");

        Some(format!(
            "You can progressively load optional toolsets with `load_toolset` and remove them with `unload_toolset`.\nAvailable toolsets:\n{catalog}\nCurrently loaded toolsets for this thread: {loaded_summary}"
        ))
    }

    /// Return the program-defined toolset catalog entries.
    pub async fn list_toolsets(&self) -> Vec<ToolsetCatalogEntry> {
        let mut entries = self
            .toolsets
            .read()
            .await
            .values()
            .map(RegisteredToolset::entry)
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        entries
    }

    /// Replace one thread runtime from persisted thread metadata.
    pub async fn rehydrate_thread(
        &self,
        thread_id: &str,
        loaded_toolsets: &[String],
    ) -> ThreadToolRuntimeSnapshot {
        self.thread_runtimes
            .replace_loaded_toolsets(thread_id, loaded_toolsets)
            .await
    }

    /// Return the loaded toolset names for one internal thread.
    pub async fn loaded_toolsets_for_thread(&self, thread_id: &str) -> Vec<String> {
        self.thread_runtimes.loaded_toolsets(thread_id).await
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
        let mut handlers = self.always_visible_handlers.write().await;
        handlers.entry(definition.name).or_insert(handler);
    }

    pub(crate) async fn toolset_registered(&self, toolset_name: &str) -> bool {
        self.toolsets.read().await.contains_key(toolset_name)
    }

    async fn ensure_tool_name_available(&self, tool_name: &str) -> Result<()> {
        if self
            .always_visible_handlers
            .read()
            .await
            .contains_key(tool_name)
        {
            bail!("tool `{tool_name}` is already registered");
        }

        let toolsets = self.toolsets.read().await;
        for toolset in toolsets.values() {
            if let RegisteredToolset::Static(definition) = toolset
                && definition.handlers.contains_key(tool_name)
            {
                bail!("tool `{tool_name}` is already owned by another toolset");
            }
        }

        Ok(())
    }

    async fn resolve_toolset_handlers(
        &self,
        toolset_name: &str,
    ) -> Result<HashMap<String, Arc<dyn ToolHandler>>> {
        let toolset = self.toolsets.read().await.get(toolset_name).cloned();
        let Some(toolset) = toolset else {
            bail!("toolset `{toolset_name}` is not registered");
        };

        match toolset {
            RegisteredToolset::Static(definition) => Ok(definition.handlers),
            RegisteredToolset::McpServer { server_name, .. } => {
                let visible_tools = self.mcp.ensure_server_tools(&server_name).await?;
                Ok(visible_tools
                    .into_iter()
                    .map(|visible_tool| {
                        let tool_name = visible_tool.definition.name.clone();
                        (
                            tool_name,
                            Arc::new(mcp::McpToolHandler::new(
                                Arc::clone(&self.mcp),
                                visible_tool,
                            )) as Arc<dyn ToolHandler>,
                        )
                    })
                    .collect())
            }
        }
    }

    async fn resolve_toolset_runtime(
        &self,
        toolset_name: &str,
    ) -> Result<Option<Arc<dyn ToolsetRuntime>>> {
        let toolset = self.toolsets.read().await.get(toolset_name).cloned();
        let Some(toolset) = toolset else {
            bail!("toolset `{toolset_name}` is not registered");
        };

        match toolset {
            RegisteredToolset::Static(definition) => Ok(definition.runtime),
            RegisteredToolset::McpServer { .. } => Ok(None),
        }
    }

    async fn sync_mcp_toolsets(&self) -> Result<()> {
        let servers = self.mcp.list_servers().await;
        let always_visible_handlers = self.always_visible_handlers.read().await;
        let mut toolsets = self.toolsets.write().await;

        toolsets.retain(|_, toolset| !matches!(toolset, RegisteredToolset::McpServer { .. }));
        for server in servers {
            if always_visible_handlers.contains_key(&server.name) {
                bail!(
                    "toolset `{}` conflicts with an always-visible tool name",
                    server.name
                );
            }
            if toolsets.contains_key(&server.name) {
                bail!("toolset `{}` is already registered", server.name);
            }

            toolsets.insert(
                server.name.clone(),
                RegisteredToolset::McpServer {
                    entry: ToolsetCatalogEntry::new(
                        server.name.clone(),
                        format!(
                            "Curated MCP toolset from server `{}` over `{}`.",
                            server.name,
                            server.transport.as_str()
                        ),
                    ),
                    server_name: server.name,
                },
            );
        }

        Ok(())
    }

    async fn sync_skill_handlers(&self) {
        let has_enabled_skills = self.skills.has_enabled_skills().await;
        let mut handlers = self.always_visible_handlers.write().await;
        if has_enabled_skills {
            handlers.insert(
                "load_skill".to_string(),
                Arc::new(skill::LoadSkillTool::new(Arc::clone(&self.skills))),
            );
        } else {
            handlers.remove("load_skill");
        }
    }

    async fn load_toolset(
        &self,
        thread_id: &str,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let args: ManageToolsetArguments = parse_tool_arguments(request, "load_toolset")?;
        let toolset_name = args.name.trim();
        if toolset_name.is_empty() {
            bail!("load_toolset requires a non-empty `name`");
        }

        self.resolve_toolset_handlers(toolset_name).await?;
        let inserted = self
            .thread_runtimes
            .load_toolset(thread_id, toolset_name)
            .await;
        let loaded_toolsets = self.thread_runtimes.loaded_toolsets(thread_id).await;

        Ok(ToolCallResult {
            content: if inserted {
                format!("Toolset `{toolset_name}` loaded for the current thread.")
            } else {
                format!("Toolset `{toolset_name}` was already loaded for the current thread.")
            },
            metadata: json!({
                "event_kind": "load_toolset",
                "toolset": toolset_name,
                "loaded_toolsets": loaded_toolsets,
                "already_loaded": !inserted,
                "approval_required": false,
                "policy_extension_point": true,
            }),
            is_error: false,
        })
    }

    async fn unload_toolset(
        &self,
        thread_id: &str,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let args: ManageToolsetArguments = parse_tool_arguments(request, "unload_toolset")?;
        let toolset_name = args.name.trim();
        if toolset_name.is_empty() {
            bail!("unload_toolset requires a non-empty `name`");
        }

        let is_loaded = self
            .thread_runtimes
            .loaded_toolsets(thread_id)
            .await
            .into_iter()
            .any(|loaded_name| loaded_name == toolset_name);
        if is_loaded && let Ok(Some(runtime)) = self.resolve_toolset_runtime(toolset_name).await {
            runtime.on_unload(thread_id).await?;
        }

        let removed = self
            .thread_runtimes
            .unload_toolset(thread_id, toolset_name)
            .await;
        let loaded_toolsets = self.thread_runtimes.loaded_toolsets(thread_id).await;

        Ok(ToolCallResult {
            content: if removed {
                format!("Toolset `{toolset_name}` unloaded for the current thread.")
            } else {
                format!("Toolset `{toolset_name}` was not loaded for the current thread.")
            },
            metadata: json!({
                "event_kind": "unload_toolset",
                "toolset": toolset_name,
                "loaded_toolsets": loaded_toolsets,
                "was_loaded": removed,
                "approval_required": false,
                "policy_extension_point": true,
            }),
            is_error: false,
        })
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
        self.registry.sync_mcp_toolsets().await?;
        Ok(snapshot)
    }

    /// Disable one managed MCP server and remove its tools from the registry.
    pub async fn disable_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let snapshot = self.registry.mcp.disable_server(name).await?;
        self.registry.sync_mcp_toolsets().await?;
        Ok(snapshot)
    }

    /// Refresh one managed MCP server and sync any tool changes.
    pub async fn refresh_server(&self, name: &str) -> Result<McpServerSnapshot> {
        let snapshot = self.registry.mcp.refresh_server(name).await?;
        self.registry.sync_mcp_toolsets().await?;
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ManageToolsetArguments {
    /// Exact program-defined toolset name to load or unload.
    name: String,
}

fn load_toolset_definition() -> ToolDefinition {
    tool_definition_from_args::<ManageToolsetArguments>(
        "load_toolset",
        "Load one program-defined toolset into the current internal thread so its tools become visible in later model steps.",
    )
}

fn unload_toolset_definition() -> ToolDefinition {
    tool_definition_from_args::<ManageToolsetArguments>(
        "unload_toolset",
        "Unload one program-defined toolset from the current internal thread so its tools disappear from later model steps.",
    )
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

fn validate_toolset_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("toolset name must not be blank");
    }
    Ok(())
}
