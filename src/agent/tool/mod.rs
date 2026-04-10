//! Shared tool traits, schemas, and registry for the built-in agent tool set.

use crate::agent::memory::{MemoryRepository, register_memory_toolset};
use crate::skill::{default_skill_roots, default_skill_roots_for_workspace};
pub mod browser;
pub mod command;
pub mod edit;
pub mod mcp;
pub mod read;
pub mod shell;
pub mod skill;
pub mod toolset;
pub mod write;

use crate::config::{AgentMcpServerConfig, AgentMcpServerTransportConfig, AgentToolConfig};
use crate::thread::Thread;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

pub use command::{
    CommandExecutionRequest, CommandExecutionResult, CommandSessionManager, CommandTaskSnapshot,
    CommandTaskStatus, CommandWriteRequest, ExecCommandTool, ListUnreadCommandTasksTool,
    WriteStdinTool,
};
pub use edit::EditTool;
pub use mcp::{
    McpServerDefinition, McpServerSnapshot, McpServerState, McpToolSnapshot, McpTransport,
};
pub use read::ReadTool;
pub use shell::ShellTool;
pub use skill::{LoadSkillTool, LoadedSkill, LoadedSkillFile, SkillManifest, SkillRegistry};
pub use toolset::ToolsetCatalogEntry;
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
    mcp: Arc<mcp::McpManager>,
    skills: Arc<skill::SkillRegistry>,
    memory_repository: Arc<MemoryRepository>,
    command_sessions: Arc<CommandSessionManager>,
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
        Self::with_workspace_root(default_workspace_root())
    }

    /// Create an empty tool registry with explicit local skill roots.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::ToolRegistry;
    /// use std::path::PathBuf;
    ///
    /// let registry = ToolRegistry::with_skill_roots(vec![PathBuf::from(".openjarvis/skills")]);
    /// assert!(registry.list().await.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_skill_roots(skill_roots: Vec<PathBuf>) -> Self {
        Self::with_workspace_root_and_skill_roots(default_workspace_root(), skill_roots)
    }

    /// Create an empty tool registry pinned to one explicit workspace root.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ToolRegistry;
    ///
    /// let registry = ToolRegistry::with_workspace_root("/tmp/openjarvis-workspace");
    /// assert!(registry.memory_repository().memory_root().ends_with(".openjarvis/memory"));
    /// ```
    pub fn with_workspace_root(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let skill_roots = default_skill_roots_for_workspace(&workspace_root);
        Self::with_workspace_root_and_skill_roots(workspace_root, skill_roots)
    }

    /// Create an empty tool registry with explicit workspace root and local skill roots.
    ///
    /// # 示例
    /// ```rust
    /// use std::path::PathBuf;
    ///
    /// use openjarvis::agent::ToolRegistry;
    ///
    /// let _registry = ToolRegistry::with_workspace_root_and_skill_roots(
    ///     "/tmp/openjarvis-workspace",
    ///     vec![PathBuf::from(".openjarvis/skills")],
    /// );
    /// ```
    pub fn with_workspace_root_and_skill_roots(
        workspace_root: impl Into<PathBuf>,
        skill_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            always_visible_handlers: RwLock::new(HashMap::new()),
            toolsets: RwLock::new(HashMap::new()),
            mcp: Arc::new(mcp::McpManager::new()),
            skills: Arc::new(skill::SkillRegistry::with_roots(skill_roots)),
            memory_repository: Arc::new(MemoryRepository::new(workspace_root.into())),
            command_sessions: Arc::new(CommandSessionManager::new()),
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
        Self::from_config_with_skill_roots(config, default_skill_roots()).await
    }

    /// Create a tool registry from config with explicit local skill roots.
    ///
    /// This exists mainly so tests can opt into deterministic roots instead of using the
    /// workspace `.openjarvis/skills` directory.
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
        // Register the current built-in tool set, including thread-scoped command sessions.
        self.register_if_missing(Arc::new(ReadTool::new())).await;
        self.register_if_missing(Arc::new(WriteTool::new())).await;
        self.register_if_missing(Arc::new(EditTool::new())).await;
        self.register_if_missing(Arc::new(ExecCommandTool::with_sessions(Arc::clone(
            &self.command_sessions,
        ))))
        .await;
        self.register_if_missing(Arc::new(WriteStdinTool::with_sessions(Arc::clone(
            &self.command_sessions,
        ))))
        .await;
        self.register_if_missing(Arc::new(ListUnreadCommandTasksTool::with_sessions(
            Arc::clone(&self.command_sessions),
        )))
        .await;
        self.register_if_missing(Arc::new(ShellTool::with_sessions(Arc::clone(
            &self.command_sessions,
        ))))
        .await;
        if !self.toolset_registered("browser").await {
            browser::register_browser_toolset(self).await?;
        }
        if !self.toolset_registered("memory").await {
            register_memory_toolset(self).await?;
        }
        self.skills.reload().await?;
        self.sync_skill_handlers().await;
        Ok(())
    }

    /// Look up a registered tool and execute the request.
    pub async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let tool_name = request.name.clone();
        let started_at = Instant::now();
        let argument_field_count = request
            .arguments
            .as_object()
            .map(|arguments| arguments.len())
            .unwrap_or_default();
        debug!(
            tool_name = %tool_name,
            argument_field_count,
            "starting always-visible tool action"
        );
        let handler = self
            .always_visible_handlers
            .read()
            .await
            .get(&request.name)
            .cloned();

        let Some(handler) = handler else {
            bail!("tool `{}` is not registered", request.name);
        };

        let result = handler
            .call_with_context(ToolCallContext::default(), request)
            .await;
        match &result {
            Ok(tool_result) => debug!(
                tool_name = %tool_name,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                is_error = tool_result.is_error,
                event_kind = ?tool_result
                    .metadata
                    .get("event_kind")
                    .and_then(|value| value.as_str()),
                "completed always-visible tool action"
            ),
            Err(error) => debug!(
                tool_name = %tool_name,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "always-visible tool action failed"
            ),
        }
        result
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

    pub(crate) async fn always_visible_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.list().await;
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    pub(crate) async fn always_visible_handler(
        &self,
        tool_name: &str,
    ) -> Option<Arc<dyn ToolHandler>> {
        self.always_visible_handlers
            .read()
            .await
            .get(tool_name)
            .cloned()
    }

    pub(crate) async fn toolset_definitions(
        &self,
        toolset_name: &str,
    ) -> Result<Vec<ToolDefinition>> {
        Ok(self
            .resolve_toolset_handlers(toolset_name)
            .await?
            .into_values()
            .map(|handler| handler.definition())
            .collect())
    }

    pub(crate) async fn toolset_handler(
        &self,
        toolset_name: &str,
        tool_name: &str,
    ) -> Result<Option<Arc<dyn ToolHandler>>> {
        Ok(self
            .resolve_toolset_handlers(toolset_name)
            .await?
            .get(tool_name)
            .cloned())
    }

    pub(crate) async fn render_toolset_catalog_prompt(
        &self,
        loaded_toolsets: &[String],
    ) -> Option<String> {
        let entries = self.list_toolsets().await;
        if entries.is_empty() {
            return None;
        }

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

    /// Compatibility wrapper that forwards thread-scoped visible tool projection back to `Thread`.
    pub async fn list_for_context(&self, thread_context: &Thread) -> Result<Vec<ToolDefinition>> {
        thread_context
            .visible_tools_with_registry(self, false)
            .await
    }

    /// Compatibility wrapper that forwards compact-aware tool projection back to `Thread`.
    pub async fn list_for_context_with_compact(
        &self,
        thread_context: &Thread,
        compact_visible: bool,
    ) -> Result<Vec<ToolDefinition>> {
        thread_context
            .visible_tools_with_registry(self, compact_visible)
            .await
    }

    /// Compatibility wrapper that forwards one thread-scoped tool call back to `Thread`.
    pub async fn call_for_context(
        &self,
        thread_context: &mut Thread,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        thread_context.call_tool_with_registry(self, request).await
    }

    /// Compatibility wrapper for loading one thread-scoped toolset through `Thread`.
    pub async fn open_tool(&self, thread_context: &mut Thread, tool_name: &str) -> Result<bool> {
        self.call_for_context(
            thread_context,
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": tool_name }),
            },
        )
        .await
        .map(|result| {
            result
                .metadata
                .get("already_loaded")
                .and_then(|value| value.as_bool())
                .map(|already_loaded| !already_loaded)
                .unwrap_or(false)
        })
    }

    /// Compatibility wrapper for unloading one thread-scoped toolset through `Thread`.
    pub async fn close_tool(&self, thread_context: &mut Thread, tool_name: &str) -> Result<bool> {
        self.call_for_context(
            thread_context,
            ToolCallRequest {
                name: "unload_toolset".to_string(),
                arguments: json!({ "name": tool_name }),
            },
        )
        .await
        .map(|result| {
            result
                .metadata
                .get("was_loaded")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
    }

    /// Compatibility wrapper that renders one thread-scoped toolset catalog prompt via `Thread`.
    pub async fn catalog_prompt_for_context(&self, thread_context: &Thread) -> Option<String> {
        thread_context
            .toolset_catalog_prompt_with_registry(self)
            .await
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

    /// Return the MCP management API exposed by this tool registry.
    pub fn mcp(&self) -> ToolRegistryMcpApi<'_> {
        ToolRegistryMcpApi { registry: self }
    }

    /// Return the local skill management API exposed by this tool registry.
    pub fn skills(&self) -> ToolRegistrySkillApi<'_> {
        ToolRegistrySkillApi { registry: self }
    }

    /// Return the shared local memory repository used by feature prompts and the memory toolset.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new();
    /// assert!(registry.memory_repository().memory_root().ends_with(".openjarvis/memory"));
    /// ```
    pub fn memory_repository(&self) -> Arc<MemoryRepository> {
        Arc::clone(&self.memory_repository)
    }

    /// Return the shared command session manager used by builtin command tools.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new();
    /// assert!(registry.command_session_manager().export_task_snapshots_blocking().is_empty());
    /// ```
    pub fn command_session_manager(&self) -> Arc<CommandSessionManager> {
        Arc::clone(&self.command_sessions)
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

    pub(crate) async fn toolset_runtime(
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
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn default_workspace_root() -> PathBuf {
    match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            warn!(
                error = %error,
                "failed to resolve current workspace root; falling back to relative `.`"
            );
            PathBuf::from(".")
        }
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
#[allow(dead_code)] // 仅用于生成 load/unload_toolset 的 schema，不直接在运行时读取字段。
struct ManageToolsetArguments {
    /// Exact program-defined toolset name to load or unload.
    name: String,
}

pub(crate) fn load_toolset_definition() -> ToolDefinition {
    tool_definition_from_args::<ManageToolsetArguments>(
        "load_toolset",
        "Load one program-defined toolset into the current internal thread so its tools become visible in later model steps.",
    )
}

pub(crate) fn unload_toolset_definition() -> ToolDefinition {
    tool_definition_from_args::<ManageToolsetArguments>(
        "unload_toolset",
        "Unload one program-defined toolset from the current internal thread so its tools disappear from later model steps.",
    )
}

pub(crate) fn compact_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "compact".to_string(),
        description: "Compact the current thread chat history into one assistant summary plus a follow-up user continue message so the task can keep going with less context.".to_string(),
        input_schema: empty_tool_input_schema(),
        source: ToolSource::Builtin,
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

fn validate_toolset_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("toolset name must not be blank");
    }
    Ok(())
}
