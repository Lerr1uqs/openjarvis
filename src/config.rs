//! Configuration loading and default values for the application, channels, and LLM provider.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};
use tracing::info;

pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str = "你是 OpenJarvis，一个有帮助、可靠、简洁的 AI 助手。请直接回答用户问题；如需要工具，基于上下文发起工具调用。";
pub const BUILTIN_MCP_SERVER_NAME: &str = "builtin_demo_stdio";
const EXTERNAL_MCP_CONFIG_RELATIVE_PATH: &str = "config/openjarvis/mcp.json";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    server: ServerConfig,
    #[serde(flatten)]
    channels: ChannelConfig,
    agent: AgentConfig,
    llm: LLMConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            channels: ChannelConfig::default(),
            agent: AgentConfig::default(),
            llm: LLMConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from `OPENJARVIS_CONFIG` or `config.yaml`.
    ///
    /// When `config/openjarvis/mcp.json` exists beside the YAML root, its MCP servers are merged
    /// into `agent.tool.mcp.servers`.
    ///
    /// # 示例
    /// ```no_run
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::load().expect("config should load");
    /// assert!(!config.llm_config().provider.trim().is_empty());
    /// ```
    pub fn load() -> Result<Self> {
        let path = env::var("OPENJARVIS_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        Self::from_path(path)
    }

    /// Load configuration from a specific YAML path, falling back to defaults when the file is
    /// missing.
    ///
    /// When `config/openjarvis/mcp.json` exists beside the YAML root, its MCP servers are merged
    /// into `agent.tool.mcp.servers`.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config =
    ///     AppConfig::from_path("missing-config.yaml").expect("missing config should use defaults");
    /// assert_eq!(config.llm_config().provider, "mock");
    /// ```
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut config = if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read config file {}", path.display()))?;
            serde_yaml::from_str::<Self>(&raw)
                .with_context(|| format!("failed to parse config file {}", path.display()))?
        } else {
            Self::default()
        };

        config.load_external_mcp_sidecar(path)?;
        config
            .validate()
            .with_context(|| format!("failed to validate config file {}", path.display()))?;
        Ok(config)
    }

    /// Return the read-only channel configuration view.
    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channels
    }

    /// Return the read-only agent runtime configuration view.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().hook_config().is_empty());
    /// ```
    pub fn agent_config(&self) -> &AgentConfig {
        &self.agent
    }

    /// Return the read-only LLM configuration view.
    pub fn llm_config(&self) -> &LLMConfig {
        &self.llm
    }

    /// Enable the demo-only builtin MCP server for local verification.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::{AppConfig, BUILTIN_MCP_SERVER_NAME};
    ///
    /// let mut config = AppConfig::default();
    /// config
    ///     .enable_builtin_mcp("openjarvis")
    ///     .expect("builtin mcp should be inserted");
    ///
    /// assert!(config.agent_config().tool_config().mcp_config().servers().contains_key(BUILTIN_MCP_SERVER_NAME));
    /// ```
    pub fn enable_builtin_mcp(&mut self, executable: impl Into<String>) -> Result<()> {
        self.agent.tool.mcp.upsert_server(
            BUILTIN_MCP_SERVER_NAME,
            AgentMcpServerConfig::stdio(
                true,
                executable,
                vec!["internal-mcp".to_string(), "demo-stdio".to_string()],
                HashMap::new(),
            ),
        );
        self.validate()
    }

    fn validate(&self) -> Result<()> {
        self.agent.validate()
    }

    fn load_external_mcp_sidecar(&mut self, config_path: &Path) -> Result<()> {
        let mcp_config_path = resolve_external_mcp_config_path(config_path);
        if !mcp_config_path.exists() {
            // Requirement: a missing sidecar should only emit a note and continue with no MCP
            // servers loaded from the external file.
            info!(
                mcp_config_path = %mcp_config_path.display(),
                "mcp sidecar config not found, continuing without external MCP servers"
            );
            return Ok(());
        }

        let raw = fs::read_to_string(&mcp_config_path).with_context(|| {
            format!(
                "failed to read mcp config file {}",
                mcp_config_path.display()
            )
        })?;
        let external_config =
            serde_json::from_str::<ExternalMcpJsonConfig>(&raw).with_context(|| {
                format!(
                    "failed to parse mcp config file {}",
                    mcp_config_path.display()
                )
            })?;
        let external_servers = external_config.into_mcp_servers().with_context(|| {
            format!(
                "failed to validate mcp config file {}",
                mcp_config_path.display()
            )
        })?;

        for (server_name, server_config) in external_servers {
            if self.agent.tool.mcp.servers.contains_key(&server_name) {
                bail!(
                    "mcp server `{server_name}` is defined in both YAML config and {}",
                    mcp_config_path.display()
                );
            }
            self.agent
                .tool
                .mcp
                .upsert_server(server_name, server_config);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3000".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ChannelConfig {
    feishu: FeishuConfig,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            feishu: FeishuConfig::default(),
        }
    }
}

impl ChannelConfig {
    /// Return the Feishu sub-configuration.
    pub fn feishu_config(&self) -> &FeishuConfig {
        &self.feishu
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FeishuConfig {
    pub mode: String,
    pub webhook_path: String,
    pub open_base_url: String,
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub encrypt_key: String,
    pub dry_run: bool,
    pub auto_start_sidecar: bool,
    pub node_bin: String,
    pub sidecar_script: String,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            mode: "long_connection".to_string(),
            webhook_path: "/webhook/feishu".to_string(),
            open_base_url: "https://open.feishu.cn".to_string(),
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            dry_run: true,
            auto_start_sidecar: true,
            node_bin: "node".to_string(),
            sidecar_script: "scripts/feishu_ws_client.mjs".to_string(),
        }
    }
}

impl FeishuConfig {
    /// Return whether the current Feishu mode should run with long connection semantics.
    pub fn is_long_connection(&self) -> bool {
        matches!(
            self.mode.as_str(),
            "long_connection" | "long-connection" | "long_connection_sdk" | "ws" | "websocket"
        )
    }
}

/// Agent-level runtime configuration loaded from YAML.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert!(config.agent_config().hook_config().is_empty());
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    hook: AgentHookConfig,
    tool: AgentToolConfig,
}

impl AgentConfig {
    /// Return the configured hook section.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().hook_config().is_empty());
    /// ```
    pub fn hook_config(&self) -> &AgentHookConfig {
        &self.hook
    }

    /// Return the configured tool runtime section.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().tool_config().mcp_config().is_empty());
    /// ```
    pub fn tool_config(&self) -> &AgentToolConfig {
        &self.tool
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.hook.validate()?;
        self.tool.validate()
    }
}

/// Tool-level runtime configuration loaded from YAML.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert!(config.agent_config().tool_config().mcp_config().is_empty());
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentToolConfig {
    mcp: AgentMcpConfig,
}

impl AgentToolConfig {
    /// Return the configured MCP subsection.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().tool_config().mcp_config().is_empty());
    /// ```
    pub fn mcp_config(&self) -> &AgentMcpConfig {
        &self.mcp
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.mcp.validate()
    }
}

/// MCP server configuration keyed by server name under `agent.tool.mcp.servers`.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert!(config.agent_config().tool_config().mcp_config().is_empty());
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentMcpConfig {
    servers: HashMap<String, AgentMcpServerConfig>,
}

impl AgentMcpConfig {
    /// Return whether no MCP server is configured.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Return the configured MCP server map.
    pub fn servers(&self) -> &HashMap<String, AgentMcpServerConfig> {
        &self.servers
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, server) in &self.servers {
            server.validate(name)?;
        }
        Ok(())
    }

    fn upsert_server(&mut self, name: impl Into<String>, server: AgentMcpServerConfig) {
        self.servers.insert(name.into(), server);
    }
}

/// One MCP server config entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentMcpServerConfig {
    pub enabled: bool,
    #[serde(flatten)]
    transport: AgentMcpServerTransportConfig,
}

impl Default for AgentMcpServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transport: AgentMcpServerTransportConfig::Stdio {
                command: String::new(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        }
    }
}

impl AgentMcpServerConfig {
    /// Create one stdio-based MCP server config entry.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AgentMcpServerConfig;
    ///
    /// let server = AgentMcpServerConfig::stdio(
    ///     true,
    ///     "openjarvis",
    ///     vec!["internal-mcp".to_string(), "demo-stdio".to_string()],
    ///     std::collections::HashMap::new(),
    /// );
    ///
    /// assert!(server.enabled);
    /// ```
    pub fn stdio(
        enabled: bool,
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            enabled,
            transport: AgentMcpServerTransportConfig::Stdio {
                command: command.into(),
                args,
                env,
            },
        }
    }

    /// Create one Streamable HTTP MCP server config entry.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AgentMcpServerConfig;
    ///
    /// let server = AgentMcpServerConfig::streamable_http(true, "http://127.0.0.1:39090/mcp");
    /// assert!(server.enabled);
    /// ```
    pub fn streamable_http(enabled: bool, url: impl Into<String>) -> Self {
        Self {
            enabled,
            transport: AgentMcpServerTransportConfig::StreamableHttp { url: url.into() },
        }
    }

    /// Return the selected transport configuration.
    pub fn transport_config(&self) -> &AgentMcpServerTransportConfig {
        &self.transport
    }

    fn validate(&self, server_name: &str) -> Result<()> {
        self.transport.validate(server_name)
    }
}

/// Transport-specific MCP server configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMcpServerTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    #[serde(rename = "streamable_http", alias = "http")]
    StreamableHttp { url: String },
}

impl AgentMcpServerTransportConfig {
    fn validate(&self, server_name: &str) -> Result<()> {
        match self {
            Self::Stdio { command, .. } => {
                if command.trim().is_empty() {
                    anyhow::bail!("mcp server `{server_name}` stdio command must not be blank");
                }
            }
            Self::StreamableHttp { url } => {
                if url.trim().is_empty() {
                    anyhow::bail!(
                        "mcp server `{server_name}` streamable_http url must not be blank"
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ExternalMcpJsonConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: HashMap<String, ExternalMcpJsonServerConfig>,
}

impl ExternalMcpJsonConfig {
    fn into_mcp_servers(self) -> Result<HashMap<String, AgentMcpServerConfig>> {
        let mut servers = HashMap::with_capacity(self.mcp_servers.len());
        for (server_name, server_config) in self.mcp_servers {
            if server_name.trim().is_empty() {
                bail!("mcp.json server name must not be blank");
            }
            servers.insert(
                server_name.clone(),
                server_config.into_agent_config(&server_name)?,
            );
        }
        Ok(servers)
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ExternalMcpJsonServerConfig {
    enabled: Option<bool>,
    transport: Option<ExternalMcpJsonTransport>,
    command: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
    url: Option<String>,
}

impl ExternalMcpJsonServerConfig {
    fn into_agent_config(self, server_name: &str) -> Result<AgentMcpServerConfig> {
        let Self {
            enabled,
            transport,
            command,
            args,
            env,
            url,
        } = self;
        let enabled = enabled.unwrap_or(true);

        match (transport, command, url) {
            (Some(ExternalMcpJsonTransport::Stdio), Some(command), None) => {
                Ok(AgentMcpServerConfig::stdio(enabled, command, args, env))
            }
            (Some(ExternalMcpJsonTransport::Stdio), None, None) => {
                bail!("mcp.json server `{server_name}` with transport `stdio` requires `command`")
            }
            (Some(ExternalMcpJsonTransport::Stdio), _, Some(_)) => bail!(
                "mcp.json server `{server_name}` with transport `stdio` must not define `url`"
            ),
            (Some(ExternalMcpJsonTransport::StreamableHttp), None, Some(url)) => {
                Ok(AgentMcpServerConfig::streamable_http(enabled, url))
            }
            (Some(ExternalMcpJsonTransport::StreamableHttp), None, None) => bail!(
                "mcp.json server `{server_name}` with transport `streamable_http` requires `url`"
            ),
            (Some(ExternalMcpJsonTransport::StreamableHttp), Some(_), _) => bail!(
                "mcp.json server `{server_name}` with transport `streamable_http` must not define `command`"
            ),
            (None, Some(command), None) => {
                Ok(AgentMcpServerConfig::stdio(enabled, command, args, env))
            }
            (None, None, Some(url)) => Ok(AgentMcpServerConfig::streamable_http(enabled, url)),
            (None, Some(_), Some(_)) => bail!(
                "mcp.json server `{server_name}` must define either `command` or `url`, not both"
            ),
            (None, None, None) => {
                bail!("mcp.json server `{server_name}` must define either `command` or `url`")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExternalMcpJsonTransport {
    Stdio,
    #[serde(rename = "streamable_http", alias = "http")]
    StreamableHttp,
}

/// Hook script configuration keyed by hook event name.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert!(config.agent_config().hook_config().is_empty());
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentHookConfig {
    pre_tool_use: Option<HookCommandConfig>,
    post_tool_use: Option<HookCommandConfig>,
    post_tool_use_failure: Option<HookCommandConfig>,
    user_prompt_submit: Option<HookCommandConfig>,
    stop: Option<HookCommandConfig>,
    subagent_start: Option<HookCommandConfig>,
    subagent_stop: Option<HookCommandConfig>,
    pre_compact: Option<HookCommandConfig>,
    permission_request: Option<HookCommandConfig>,
    notification: Option<HookCommandConfig>,
    session_start: Option<HookCommandConfig>,
    session_end: Option<HookCommandConfig>,
    setup: Option<HookCommandConfig>,
    teammate_idle: Option<HookCommandConfig>,
    task_completed: Option<HookCommandConfig>,
    config_change: Option<HookCommandConfig>,
    worktree_create: Option<HookCommandConfig>,
    worktree_remove: Option<HookCommandConfig>,
}

impl AgentHookConfig {
    /// Return whether no hook script has been configured.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().hook_config().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.configured_commands().is_empty()
    }

    pub(crate) fn configured_commands(&self) -> Vec<(&'static str, &HookCommandConfig)> {
        let mut commands = Vec::new();
        push_command(&mut commands, "pre_tool_use", self.pre_tool_use.as_ref());
        push_command(&mut commands, "post_tool_use", self.post_tool_use.as_ref());
        push_command(
            &mut commands,
            "post_tool_use_failure",
            self.post_tool_use_failure.as_ref(),
        );
        push_command(
            &mut commands,
            "user_prompt_submit",
            self.user_prompt_submit.as_ref(),
        );
        push_command(&mut commands, "stop", self.stop.as_ref());
        push_command(
            &mut commands,
            "subagent_start",
            self.subagent_start.as_ref(),
        );
        push_command(&mut commands, "subagent_stop", self.subagent_stop.as_ref());
        push_command(&mut commands, "pre_compact", self.pre_compact.as_ref());
        push_command(
            &mut commands,
            "permission_request",
            self.permission_request.as_ref(),
        );
        push_command(&mut commands, "notification", self.notification.as_ref());
        push_command(&mut commands, "session_start", self.session_start.as_ref());
        push_command(&mut commands, "session_end", self.session_end.as_ref());
        push_command(&mut commands, "setup", self.setup.as_ref());
        push_command(&mut commands, "teammate_idle", self.teammate_idle.as_ref());
        push_command(
            &mut commands,
            "task_completed",
            self.task_completed.as_ref(),
        );
        push_command(&mut commands, "config_change", self.config_change.as_ref());
        push_command(
            &mut commands,
            "worktree_create",
            self.worktree_create.as_ref(),
        );
        push_command(
            &mut commands,
            "worktree_remove",
            self.worktree_remove.as_ref(),
        );
        commands
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (event_name, command) in self.configured_commands() {
            command.validate(event_name)?;
        }

        Ok(())
    }
}

/// One hook command represented as `[program, arg1, arg2, ...]`.
///
/// # 示例
/// ```rust
/// let command: openjarvis::config::HookCommandConfig =
///     serde_yaml::from_str("[\"echo\", \"hello\"]").expect("command should parse");
///
/// assert_eq!(command.parts(), ["echo", "hello"]);
/// ```
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct HookCommandConfig(Vec<String>);

impl HookCommandConfig {
    /// Return the configured command parts in order.
    ///
    /// # 示例
    /// ```rust
    /// let command: openjarvis::config::HookCommandConfig =
    ///     serde_yaml::from_str("[\"echo\", \"hello\"]").expect("command should parse");
    ///
    /// assert_eq!(command.parts(), ["echo", "hello"]);
    /// ```
    pub fn parts(&self) -> &[String] {
        &self.0
    }

    fn validate(&self, event_name: &str) -> Result<()> {
        if self.0.is_empty() {
            anyhow::bail!("{event_name} hook command must not be empty");
        }

        for (index, part) in self.0.iter().enumerate() {
            if part.trim().is_empty() {
                anyhow::bail!("{event_name} hook command part at index {index} must not be blank");
            }
        }

        Ok(())
    }
}

fn push_command<'a>(
    commands: &mut Vec<(&'static str, &'a HookCommandConfig)>,
    event_name: &'static str,
    command: Option<&'a HookCommandConfig>,
) {
    if let Some(command) = command {
        commands.push((event_name, command));
    }
}

fn resolve_external_mcp_config_path(config_path: &Path) -> PathBuf {
    let config_root = config_path.parent().unwrap_or_else(|| Path::new("."));
    config_root.join(EXTERNAL_MCP_CONFIG_RELATIVE_PATH)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LLMConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub api_key_path: PathBuf,
    pub mock_response: String,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            provider: "mock".to_string(),
            model: "mock-received".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            api_key_path: PathBuf::new(),
            mock_response: "[openjarvis][DEBUG] 测试回复".to_string(),
        }
    }
}
