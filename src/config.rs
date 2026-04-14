//! Configuration loading and default values for the application, channels, and LLM provider.

use crate::thread::Features;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env, fmt, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str = "你是 OpenJarvis，一个有帮助、可靠、简洁的 AI 助手。请直接回答用户问题；如需要工具，基于上下文发起工具调用。当你需要回复图片时，必须在回复中使用 `#!openjarvis[image:/绝对路径/图片.png]` 语法传递图片绝对路径，可以与普通文本同时输出；不要改写该语法，不要输出相对路径，也不要解释这个协议本身。";
pub const BUILTIN_MCP_SERVER_NAME: &str = "builtin_demo_stdio";
const EXTERNAL_MCP_CONFIG_RELATIVE_PATH: &str = "config/openjarvis/mcp.json";
const DEFAULT_CONTEXT_WINDOW_TOKENS: usize = 8_192;
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 1_024;
const KIMI_K2_5_CONTEXT_WINDOW_TOKENS: usize = 262_144;
const KIMI_K2_5_MAX_OUTPUT_TOKENS: usize = 32_768;
static GLOBAL_APP_CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Install one process-wide read-only app config snapshot.
///
/// The input config is validated before installation and can only be installed once during the
/// current process lifetime.
///
/// # 示例
/// ```rust
/// use openjarvis::config::{AppConfig, install_global_config};
///
/// let config = AppConfig::builder_for_test()
///     .build()
///     .expect("test config should build");
/// let installed = install_global_config(config).expect("config should install once");
///
/// assert_eq!(installed.llm_config().effective_protocol(), "mock");
/// ```
pub fn install_global_config(config: AppConfig) -> Result<&'static AppConfig> {
    config
        .validate()
        .context("failed to validate app config before global installation")?;
    if GLOBAL_APP_CONFIG.set(config).is_err() {
        bail!("global app config has already been installed");
    }

    let installed = GLOBAL_APP_CONFIG
        .get()
        .expect("global app config should be readable immediately after installation");
    let resolved_llm = installed
        .llm_config()
        .resolve_active_provider()
        .context("global config should expose one resolved active llm provider")?;
    info!(
        llm_protocol = resolved_llm.effective_protocol(),
        llm_provider = %resolved_llm.name,
        llm_model = %resolved_llm.model,
        builtin_mcp_enabled = installed
            .agent_config()
            .tool_config()
            .mcp_config()
            .servers()
            .contains_key(BUILTIN_MCP_SERVER_NAME),
        "installed global read-only app config"
    );
    Ok(installed)
}

/// Return the installed global app config snapshot, or fail fast when startup has not installed it
/// yet.
///
/// Production startup paths should call [`install_global_config`] before using this accessor.
/// Tests and embedded callers that do not want global state should keep using explicit
/// `from_config(...)` style APIs instead.
///
/// # 示例
/// ```rust
/// use openjarvis::config::{AppConfig, global_config, install_global_config};
///
/// let config = AppConfig::builder_for_test()
///     .build()
///     .expect("test config should build");
/// install_global_config(config).expect("config should install");
///
/// assert_eq!(global_config().llm_config().effective_protocol(), "mock");
/// ```
pub fn global_config() -> &'static AppConfig {
    GLOBAL_APP_CONFIG.get().expect(
        "global app config is not installed; call install_global_config before accessing it",
    )
}

/// Return the installed global app config snapshot when startup has already installed it.
///
/// This probe helper exists for code paths that need to detect initialization state without
/// panicking.
///
/// # 示例
/// ```rust
/// use openjarvis::config::try_global_config;
///
/// assert!(try_global_config().is_none());
/// ```
pub fn try_global_config() -> Option<&'static AppConfig> {
    GLOBAL_APP_CONFIG.get()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    server: ServerConfig,
    logging: LoggingConfig,
    session: SessionConfig,
    #[serde(flatten)]
    channels: ChannelConfig,
    agent: AgentConfig,
    llm: LLMConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            session: SessionConfig::default(),
            channels: ChannelConfig::default(),
            agent: AgentConfig::default(),
            llm: LLMConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from `OPENJARVIS_CONFIG` or `config.yaml`.
    ///
    /// This is the default startup entrypoint. It applies the same validation, relative-path
    /// resolution, and optional `config/openjarvis/mcp.json` sidecar merge behavior as
    /// [`AppConfig::from_yaml_path`].
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
        Self::from_yaml_path(path)
    }

    /// Load configuration from one specific YAML path, falling back to defaults when the file is
    /// missing.
    ///
    /// Compared with [`AppConfig::from_yaml_str`], this entrypoint also resolves relative logging
    /// and session paths against the YAML location and attempts to merge the optional
    /// `config/openjarvis/mcp.json` sidecar beside the YAML root.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config =
    ///     AppConfig::from_yaml_path("missing-config.yaml").expect("missing config should use defaults");
    /// assert_eq!(config.llm_config().provider, "unknown");
    /// assert_eq!(config.llm_config().effective_protocol(), "mock");
    /// ```
    pub fn from_yaml_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut config = if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read config file {}", path.display()))?;
            serde_yaml::from_str::<Self>(&raw)
                .with_context(|| format!("failed to parse config file {}", path.display()))?
        } else {
            Self::default()
        };

        config.resolve_paths(path);
        config.load_external_mcp_sidecar(path)?;
        config
            .validate()
            .with_context(|| format!("failed to validate config file {}", path.display()))?;
        Ok(config)
    }

    /// Parse configuration directly from one YAML string.
    ///
    /// Compared with [`AppConfig::from_yaml_path`], this entrypoint validates the parsed config but
    /// does not resolve relative filesystem paths and does not load any external MCP sidecar file
    /// because it has no filesystem anchor.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::from_yaml_str(
    ///     r#"
    /// llm:
    ///   protocol: "mock"
    ///   mock_response: "pong"
    /// "#,
    /// )
    /// .expect("yaml string should parse");
    ///
    /// assert_eq!(config.llm_config().mock_response, "pong");
    /// ```
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let config = serde_yaml::from_str::<Self>(yaml)
            .context("failed to parse config from yaml string")?;
        config
            .validate()
            .context("failed to validate config from yaml string")?;
        Ok(config)
    }

    /// Return one minimal validated config builder for unit tests and embedded construction.
    ///
    /// This entrypoint starts from [`AppConfig::default`] and keeps configuration explicit inside
    /// the current test without touching the process-wide global snapshot.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::{AppConfig, LLMConfig};
    ///
    /// let config = AppConfig::builder_for_test()
    ///     .llm(LLMConfig {
    ///         protocol: "mock".to_string(),
    ///         mock_response: "builder".to_string(),
    ///         ..LLMConfig::default()
    ///     })
    ///     .build()
    ///     .expect("builder config should validate");
    ///
    /// assert_eq!(config.llm_config().mock_response, "builder");
    /// ```
    pub fn builder_for_test() -> AppConfigBuilderForTest {
        AppConfigBuilderForTest::default()
    }

    #[doc(hidden)]
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_yaml_path(path)
    }

    /// Return the read-only channel configuration view.
    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channels
    }

    /// Return the read-only logging configuration view.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.logging_config().file_config().enabled());
    /// ```
    pub fn logging_config(&self) -> &LoggingConfig {
        &self.logging
    }

    /// Return the read-only session persistence configuration view.
    pub fn session_config(&self) -> &SessionConfig {
        &self.session
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

    /// Enable the demo-only builtin MCP server for local verification before global installation.
    ///
    /// This is only a startup-phase override for the to-be-installed config snapshot. Callers
    /// should finish this mutation before [`install_global_config`] and should not treat it as a
    /// runtime writable config interface.
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
        self.logging.validate()?;
        self.channels.validate()?;
        self.session.validate()?;
        self.llm.validate()?;
        self.agent.validate()
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        self.logging.resolve_paths(config_path);
        self.session.resolve_paths(config_path);
        self.agent.resolve_paths(config_path);
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

/// Builder used by tests to assemble one validated [`AppConfig`] without depending on global
/// process state.
#[derive(Debug, Clone)]
pub struct AppConfigBuilderForTest {
    config: AppConfig,
}

impl Default for AppConfigBuilderForTest {
    fn default() -> Self {
        Self {
            config: AppConfig::default(),
        }
    }
}

impl AppConfigBuilderForTest {
    /// Replace the logging section used by the test config under construction.
    pub fn logging(mut self, logging: LoggingConfig) -> Self {
        self.config.logging = logging;
        self
    }

    /// Replace the channel section used by the test config under construction.
    pub fn channels(mut self, channels: ChannelConfig) -> Self {
        self.config.channels = channels;
        self
    }

    /// Replace the session section used by the test config under construction.
    pub fn session(mut self, session: SessionConfig) -> Self {
        self.config.session = session;
        self
    }

    /// Replace the agent section used by the test config under construction.
    pub fn agent(mut self, agent: AgentConfig) -> Self {
        self.config.agent = agent;
        self
    }

    /// Replace the LLM section used by the test config under construction.
    pub fn llm(mut self, llm: LLMConfig) -> Self {
        self.config.llm = llm;
        self
    }

    /// Finish the builder and validate the resulting config snapshot.
    ///
    /// This builder path intentionally does not resolve relative paths from disk and does not load
    /// external MCP sidecar files; tests should set those values explicitly when they care.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::{AppConfig, LLMConfig};
    ///
    /// let config = AppConfig::builder_for_test()
    ///     .llm(LLMConfig {
    ///         protocol: "mock".to_string(),
    ///         mock_response: "pong".to_string(),
    ///         ..LLMConfig::default()
    ///     })
    ///     .build()
    ///     .expect("builder config should validate");
    ///
    /// assert_eq!(config.llm_config().mock_response, "pong");
    /// ```
    pub fn build(self) -> Result<AppConfig> {
        self.config
            .validate()
            .context("failed to validate app config built for test")?;
        Ok(self.config)
    }
}

/// Logging configuration loaded from `logging`.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert_eq!(config.logging_config().level_filter(), "info");
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    level: String,
    stderr: bool,
    stderr_ansi: bool,
    file: FileLoggingConfig,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            stderr: true,
            stderr_ansi: false,
            file: FileLoggingConfig::default(),
        }
    }
}

impl LoggingConfig {
    /// Return the default tracing filter expression used when `RUST_LOG` is absent.
    pub fn level_filter(&self) -> &str {
        &self.level
    }

    /// Return whether logs should also be written to stderr.
    pub fn stderr_enabled(&self) -> bool {
        self.stderr
    }

    /// Return whether stderr output should keep ANSI colors.
    pub fn stderr_ansi(&self) -> bool {
        self.stderr_ansi
    }

    /// Return the file sink configuration.
    pub fn file_config(&self) -> &FileLoggingConfig {
        &self.file
    }

    pub(crate) fn set_level_filter(&mut self, level: impl Into<String>) {
        self.level = level.into();
    }

    pub(crate) fn set_stderr_enabled(&mut self, enabled: bool) {
        self.stderr = enabled;
    }

    pub(crate) fn set_stderr_ansi(&mut self, enabled: bool) {
        self.stderr_ansi = enabled;
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.level.trim().is_empty() {
            bail!("logging.level must not be blank");
        }
        EnvFilter::try_new(self.level.trim()).with_context(|| {
            format!(
                "logging.level `{}` is not a valid tracing filter expression",
                self.level
            )
        })?;
        if !self.stderr && !self.file.enabled {
            bail!("logging requires at least one enabled sink: stderr or file");
        }
        self.file.validate()
    }

    pub(crate) fn resolve_paths(&mut self, config_path: &Path) {
        self.file.resolve_paths(config_path);
    }
}

/// File sink configuration for local persistent logs.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert_eq!(config.logging_config().file_config().rotation(), openjarvis::config::LogRotation::Daily);
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileLoggingConfig {
    enabled: bool,
    directory: PathBuf,
    rotation: LogRotation,
    filename_prefix: String,
    filename_suffix: String,
    max_files: usize,
}

impl Default for FileLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("logs"),
            rotation: LogRotation::Daily,
            filename_prefix: "openjarvis".to_string(),
            filename_suffix: "log".to_string(),
            max_files: 7,
        }
    }
}

impl FileLoggingConfig {
    /// Return whether file logging is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Return the directory used for local log files.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Return the rolling strategy used for local log files.
    pub fn rotation(&self) -> LogRotation {
        self.rotation
    }

    /// Return the configured file-name prefix.
    pub fn filename_prefix(&self) -> &str {
        &self.filename_prefix
    }

    /// Return the configured file-name suffix.
    pub fn filename_suffix(&self) -> &str {
        &self.filename_suffix
    }

    /// Return the maximum retained file count. `0` disables pruning.
    pub fn max_files(&self) -> usize {
        self.max_files
    }

    fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.directory.as_os_str().is_empty() {
            bail!("logging.file.directory must not be blank");
        }
        if self.filename_prefix.trim().is_empty() {
            bail!("logging.file.filename_prefix must not be blank");
        }

        Ok(())
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        if self.directory.is_absolute() || self.directory.as_os_str().is_empty() {
            return;
        }

        let config_root = config_path.parent().unwrap_or_else(|| Path::new("."));
        self.directory = config_root.join(&self.directory);
    }
}

/// Rolling strategy used for local log files.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogRotation {
    Minutely,
    Hourly,
    Daily,
    Never,
}

impl Default for LogRotation {
    fn default() -> Self {
        Self::Daily
    }
}

impl fmt::Display for LogRotation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Minutely => "minutely",
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Never => "never",
        })
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

    pub(crate) fn validate(&self) -> Result<()> {
        self.feishu.validate()
    }
}

/// Session persistence configuration loaded from `session`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    persistence: SessionPersistenceConfig,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            persistence: SessionPersistenceConfig::default(),
        }
    }
}

impl SessionConfig {
    /// Return the configured session persistence subsection.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::{AppConfig, SessionStoreBackend};
    ///
    /// let config = AppConfig::default();
    /// assert_eq!(
    ///     config.session_config().persistence_config().backend(),
    ///     SessionStoreBackend::Sqlite
    /// );
    /// ```
    pub fn persistence_config(&self) -> &SessionPersistenceConfig {
        &self.persistence
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.persistence.validate()
    }

    pub(crate) fn resolve_paths(&mut self, config_path: &Path) {
        self.persistence.resolve_paths(config_path);
    }
}

/// Session persistence backend and backend-specific settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionPersistenceConfig {
    backend: SessionStoreBackend,
    sqlite: SessionSqliteConfig,
}

impl Default for SessionPersistenceConfig {
    fn default() -> Self {
        Self {
            backend: SessionStoreBackend::Sqlite,
            sqlite: SessionSqliteConfig::default(),
        }
    }
}

impl SessionPersistenceConfig {
    /// Return the selected session persistence backend.
    pub fn backend(&self) -> SessionStoreBackend {
        self.backend
    }

    /// Return the SQLite-specific session persistence configuration.
    pub fn sqlite_config(&self) -> &SessionSqliteConfig {
        &self.sqlite
    }

    fn validate(&self) -> Result<()> {
        match self.backend {
            SessionStoreBackend::Memory => Ok(()),
            SessionStoreBackend::Sqlite => self.sqlite.validate(),
        }
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        self.sqlite.resolve_paths(config_path);
    }
}

/// Supported backends for thread-context persistence.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStoreBackend {
    Memory,
    Sqlite,
}

/// SQLite-specific session persistence settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionSqliteConfig {
    path: PathBuf,
}

impl Default for SessionSqliteConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("data/openjarvis/session.sqlite3"),
        }
    }
}

impl SessionSqliteConfig {
    /// Return the SQLite database path used for thread persistence.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn validate(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            bail!("session.persistence.sqlite.path must not be blank");
        }
        Ok(())
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        if self.path.is_absolute() || self.path.as_os_str().is_empty() {
            return;
        }

        let config_root = config_path.parent().unwrap_or_else(|| Path::new("."));
        self.path = config_root.join(&self.path);
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
    users: HashMap<String, FeishuUserConfig>,
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
            users: HashMap::new(),
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

    /// Return the configured per-user channel overrides.
    pub fn users(&self) -> &HashMap<String, FeishuUserConfig> {
        &self.users
    }

    /// Return the explicit thread feature override for one Feishu user when configured.
    pub fn user_features(&self, user_id: &str) -> Option<&Features> {
        self.users.get(user_id).and_then(FeishuUserConfig::features)
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

/// One Feishu user-scoped override entry.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct FeishuUserConfig {
    features: Option<Features>,
}

impl FeishuUserConfig {
    /// Return the explicit feature override for this user when configured.
    pub fn features(&self) -> Option<&Features> {
        self.features.as_ref()
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
    compact: AgentCompactConfig,
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

    /// Return the configured compact runtime section.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(!config.agent_config().compact_config().enabled());
    /// ```
    pub fn compact_config(&self) -> &AgentCompactConfig {
        &self.compact
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.hook.validate()?;
        self.tool.validate()?;
        self.compact.validate()
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        self.tool.resolve_paths(config_path);
    }
}

/// Compact runtime configuration loaded from `agent.compact`.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert_eq!(
///     config.agent_config().compact_config().runtime_threshold_ratio(),
///     0.85
/// );
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCompactConfig {
    enabled: bool,
    auto_compact: bool,
    runtime_threshold_ratio: f64,
    tool_visible_threshold_ratio: f64,
    reserved_output_tokens: Option<usize>,
    mock_compacted_assistant: Option<String>,
}

impl Default for AgentCompactConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_compact: false,
            runtime_threshold_ratio: 0.85,
            tool_visible_threshold_ratio: 0.70,
            reserved_output_tokens: None,
            mock_compacted_assistant: None,
        }
    }
}

impl AgentCompactConfig {
    /// Return whether runtime-managed compact is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Return whether model-assisted auto-compact is enabled.
    ///
    /// 启用后 `compact` 工具会始终暴露给模型，且每次 generate 都会注入当前上下文容量提示。
    pub fn auto_compact(&self) -> bool {
        self.auto_compact
    }

    /// Return the hard runtime compact trigger ratio.
    pub fn runtime_threshold_ratio(&self) -> f64 {
        self.runtime_threshold_ratio
    }

    /// Return the soft threshold used to upgrade the auto-compact prompt into an early-warning hint.
    ///
    /// 这个阈值不控制 `compact` 工具是否可见，也不控制是否注入提示；
    /// 它只控制当前上下文使用率较高时，是否在提示中更明确地建议模型提前调用 `compact`。
    pub fn tool_visible_threshold_ratio(&self) -> f64 {
        self.tool_visible_threshold_ratio
    }

    /// Return the reserved output token budget for one LLM request.
    pub fn reserved_output_tokens(&self) -> usize {
        self.reserved_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
    }

    /// Return the explicitly configured legacy reserved output token budget when present.
    pub fn configured_reserved_output_tokens(&self) -> Option<usize> {
        self.reserved_output_tokens
    }

    /// Return the optional static compact summary used for deterministic mock compaction.
    pub fn mock_compacted_assistant(&self) -> Option<&str> {
        self.mock_compacted_assistant.as_deref()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_ratio(
            self.runtime_threshold_ratio,
            "agent.compact.runtime_threshold_ratio",
        )?;
        validate_ratio(
            self.tool_visible_threshold_ratio,
            "agent.compact.tool_visible_threshold_ratio",
        )?;
        if self.tool_visible_threshold_ratio > self.runtime_threshold_ratio {
            bail!(
                "agent.compact.tool_visible_threshold_ratio must be less than or equal to agent.compact.runtime_threshold_ratio"
            );
        }
        if self.auto_compact && !self.enabled {
            bail!("agent.compact.auto_compact requires agent.compact.enabled=true");
        }
        if self
            .reserved_output_tokens
            .is_some_and(|reserved_output_tokens| reserved_output_tokens == 0)
        {
            bail!("agent.compact.reserved_output_tokens must be greater than 0");
        }
        if self
            .mock_compacted_assistant
            .as_deref()
            .is_some_and(|summary| summary.trim().is_empty())
        {
            bail!("agent.compact.mock_compacted_assistant must not be blank");
        }

        Ok(())
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
    browser: AgentBrowserToolConfig,
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

    /// Return the configured browser runtime subsection.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::AppConfig;
    ///
    /// let config = AppConfig::default();
    /// assert!(config.agent_config().tool_config().browser_config().cookies_state_file().is_none());
    /// ```
    pub fn browser_config(&self) -> &AgentBrowserToolConfig {
        &self.browser
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.mcp.validate()?;
        self.browser.validate()
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        self.browser.resolve_paths(config_path);
    }
}

/// Browser runtime configuration loaded from `agent.tool.browser`.
///
/// # 示例
/// ```rust
/// use openjarvis::config::AppConfig;
///
/// let config = AppConfig::default();
/// assert!(!config.agent_config().tool_config().browser_config().load_cookies_on_open());
/// assert!(!config.agent_config().tool_config().browser_config().save_cookies_on_close());
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentBrowserToolConfig {
    cookies_state_file: Option<PathBuf>,
    load_cookies_on_open: bool,
    save_cookies_on_close: bool,
}

impl AgentBrowserToolConfig {
    /// Return the optional cookies state file path used for browser session reuse.
    pub fn cookies_state_file(&self) -> Option<&Path> {
        self.cookies_state_file.as_deref()
    }

    /// Return whether launch-mode browser open should auto-load cookies from the configured file.
    pub fn load_cookies_on_open(&self) -> bool {
        self.load_cookies_on_open
    }

    /// Return whether launch-mode browser close should auto-save cookies to the configured file.
    pub fn save_cookies_on_close(&self) -> bool {
        self.save_cookies_on_close
    }

    fn validate(&self) -> Result<()> {
        if (self.load_cookies_on_open || self.save_cookies_on_close)
            && self.cookies_state_file.is_none()
        {
            bail!(
                "agent.tool.browser.cookies_state_file is required when browser cookies auto-load or auto-save is enabled"
            );
        }
        if self
            .cookies_state_file
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            bail!("agent.tool.browser.cookies_state_file must not be blank");
        }
        Ok(())
    }

    fn resolve_paths(&mut self, config_path: &Path) {
        let Some(path) = self.cookies_state_file.as_mut() else {
            return;
        };
        if path.is_absolute() || path.as_os_str().is_empty() {
            return;
        }

        let config_root = config_path.parent().unwrap_or_else(|| Path::new("."));
        *path = config_root.join(&*path);
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

fn default_deserialized_llm_protocol() -> String {
    String::new()
}

fn default_runtime_llm_protocol() -> String {
    "mock".to_string()
}

fn default_llm_provider() -> String {
    "unknown".to_string()
}

fn default_llm_model() -> String {
    "mock-received".to_string()
}

fn default_llm_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_llm_api_key() -> String {
    String::new()
}

fn default_llm_api_key_path() -> PathBuf {
    PathBuf::new()
}

fn default_llm_mock_response() -> String {
    "[openjarvis][DEBUG] 测试回复".to_string()
}

fn default_llm_context_window_tokens() -> Option<usize> {
    None
}

fn default_llm_max_output_tokens() -> Option<usize> {
    None
}

fn default_llm_tokenizer() -> String {
    "chars_div4".to_string()
}

fn default_llm_system_prompt() -> String {
    DEFAULT_ASSISTANT_SYSTEM_PROMPT.to_string()
}

fn default_llm_headers() -> HashMap<String, String> {
    HashMap::new()
}

#[derive(Debug, Default, Clone)]
struct RawLLMProviderDefaults {
    protocol: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    api_key_path: Option<PathBuf>,
    mock_response: Option<String>,
    context_window_tokens: Option<usize>,
    max_output_tokens: Option<usize>,
    tokenizer: Option<String>,
}

impl RawLLMProviderDefaults {
    fn from_raw_llm_config(raw: &RawLLMConfig) -> Self {
        Self {
            protocol: raw.protocol.clone(),
            model: raw.model.clone(),
            base_url: raw.base_url.clone(),
            api_key: raw.api_key.clone(),
            api_key_path: raw.api_key_path.clone(),
            mock_response: raw.mock_response.clone(),
            context_window_tokens: raw.context_window_tokens,
            max_output_tokens: raw.max_output_tokens,
            tokenizer: raw.tokenizer.clone(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawLLMProviderProfileConfig {
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_path: Option<PathBuf>,
    #[serde(default)]
    mock_response: Option<String>,
    #[serde(default)]
    context_window_tokens: Option<usize>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    tokenizer: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

impl RawLLMProviderProfileConfig {
    fn into_profile(self, defaults: &RawLLMProviderDefaults) -> LLMProviderProfileConfig {
        LLMProviderProfileConfig {
            protocol: self
                .protocol
                .or_else(|| defaults.protocol.clone())
                .unwrap_or_else(default_deserialized_llm_protocol),
            model: self
                .model
                .or_else(|| defaults.model.clone())
                .unwrap_or_else(default_llm_model),
            base_url: self
                .base_url
                .or_else(|| defaults.base_url.clone())
                .unwrap_or_else(default_llm_base_url),
            api_key: self
                .api_key
                .or_else(|| defaults.api_key.clone())
                .unwrap_or_else(default_llm_api_key),
            api_key_path: self
                .api_key_path
                .or_else(|| defaults.api_key_path.clone())
                .unwrap_or_else(default_llm_api_key_path),
            mock_response: self
                .mock_response
                .or_else(|| defaults.mock_response.clone())
                .unwrap_or_else(default_llm_mock_response),
            context_window_tokens: self
                .context_window_tokens
                .or(defaults.context_window_tokens),
            max_output_tokens: self.max_output_tokens.or(defaults.max_output_tokens),
            tokenizer: self
                .tokenizer
                .or_else(|| defaults.tokenizer.clone())
                .unwrap_or_else(default_llm_tokenizer),
            headers: self.headers,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawNamedLLMProviderProfileConfig {
    name: String,
    #[serde(flatten)]
    profile: RawLLMProviderProfileConfig,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawLLMProviders {
    Map(HashMap<String, RawLLMProviderProfileConfig>),
    List(Vec<RawNamedLLMProviderProfileConfig>),
}

impl Default for RawLLMProviders {
    fn default() -> Self {
        Self::Map(HashMap::new())
    }
}

impl RawLLMProviders {
    fn into_profiles(
        self,
        defaults: &RawLLMProviderDefaults,
    ) -> std::result::Result<(HashMap<String, LLMProviderProfileConfig>, bool), String> {
        match self {
            Self::Map(profiles) => Ok((
                profiles
                    .into_iter()
                    .map(|(name, profile)| (name, profile.into_profile(defaults)))
                    .collect(),
                false,
            )),
            Self::List(entries) => {
                let mut profiles = HashMap::with_capacity(entries.len());
                for entry in entries {
                    let name = entry.name.trim();
                    if name.is_empty() {
                        return Err("llm.providers[].name must not be blank".to_string());
                    }
                    if profiles
                        .insert(name.to_string(), entry.profile.into_profile(defaults))
                        .is_some()
                    {
                        return Err(format!(
                            "llm.providers contains duplicated provider `{name}`"
                        ));
                    }
                }
                Ok((profiles, true))
            }
        }
    }
}

/// One named LLM provider profile declared under `llm.providers`.
///
/// # 示例
/// ```rust
/// use openjarvis::config::LLMProviderProfileConfig;
///
/// let profile: LLMProviderProfileConfig = serde_yaml::from_str(
///     r#"
/// protocol: "openai_responses"
/// model: "gpt-5-mini"
/// base_url: "https://api.openai.com/v1"
/// api_key: "test-key"
/// headers:
///   X-Trace-Id: "demo"
/// "#,
/// )
/// .expect("profile should parse");
///
/// assert_eq!(profile.effective_protocol(), "openai_responses");
/// assert_eq!(profile.headers["X-Trace-Id"], "demo");
/// ```
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LLMProviderProfileConfig {
    #[serde(default = "default_deserialized_llm_protocol")]
    pub protocol: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_api_key")]
    pub api_key: String,
    #[serde(default = "default_llm_api_key_path")]
    pub api_key_path: PathBuf,
    #[serde(default = "default_llm_mock_response")]
    pub mock_response: String,
    #[serde(default = "default_llm_context_window_tokens")]
    pub context_window_tokens: Option<usize>,
    #[serde(default = "default_llm_max_output_tokens")]
    pub max_output_tokens: Option<usize>,
    #[serde(default = "default_llm_tokenizer")]
    pub tokenizer: String,
    #[serde(default = "default_llm_headers")]
    pub headers: HashMap<String, String>,
}

impl Default for LLMProviderProfileConfig {
    fn default() -> Self {
        Self {
            protocol: default_runtime_llm_protocol(),
            model: default_llm_model(),
            base_url: default_llm_base_url(),
            api_key: default_llm_api_key(),
            api_key_path: default_llm_api_key_path(),
            mock_response: default_llm_mock_response(),
            context_window_tokens: default_llm_context_window_tokens(),
            max_output_tokens: default_llm_max_output_tokens(),
            tokenizer: default_llm_tokenizer(),
            headers: default_llm_headers(),
        }
    }
}

impl LLMProviderProfileConfig {
    /// Return the normalized protocol used to choose the concrete LLM transport implementation.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMProviderProfileConfig;
    ///
    /// let profile = LLMProviderProfileConfig {
    ///     protocol: "openai_chat_completions".to_string(),
    ///     ..LLMProviderProfileConfig::default()
    /// };
    ///
    /// assert_eq!(profile.effective_protocol(), "openai_compatible");
    /// ```
    pub fn effective_protocol(&self) -> &'static str {
        normalize_llm_protocol(&self.protocol)
    }

    /// Return the effective context window tokens for the configured profile.
    pub fn context_window_tokens(&self) -> usize {
        effective_context_window_tokens(&self.model, self.context_window_tokens)
    }

    /// Return the effective max output tokens for the configured profile.
    pub fn max_output_tokens(&self) -> usize {
        effective_max_output_tokens(&self.model, self.max_output_tokens)
    }

    fn validate(&self, field_prefix: &str) -> Result<()> {
        validate_llm_profile(
            &self.protocol,
            &self.model,
            self.context_window_tokens,
            self.max_output_tokens,
            &self.tokenizer,
            &self.headers,
            field_prefix,
        )
    }
}

/// Resolved active LLM provider view used by runtime assembly and logging.
///
/// # 示例
/// ```rust
/// use openjarvis::config::{LLMProviderProfileConfig, ResolvedLLMProviderConfig};
///
/// let resolved = ResolvedLLMProviderConfig::from_profile(
///     "demo",
///     &LLMProviderProfileConfig {
///         protocol: "openai_responses".to_string(),
///         model: "gpt-5-mini".to_string(),
///         ..LLMProviderProfileConfig::default()
///     },
/// );
///
/// assert_eq!(resolved.name, "demo");
/// assert_eq!(resolved.effective_protocol(), "openai_responses");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLLMProviderConfig {
    pub name: String,
    pub protocol: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub api_key_path: PathBuf,
    pub mock_response: String,
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub tokenizer: String,
    pub headers: HashMap<String, String>,
}

impl ResolvedLLMProviderConfig {
    /// Build one runtime provider view from a named provider profile.
    pub fn from_profile(name: impl Into<String>, profile: &LLMProviderProfileConfig) -> Self {
        Self {
            name: name.into(),
            protocol: profile.protocol.clone(),
            model: profile.model.clone(),
            base_url: profile.base_url.clone(),
            api_key: profile.api_key.clone(),
            api_key_path: profile.api_key_path.clone(),
            mock_response: profile.mock_response.clone(),
            context_window_tokens: profile.context_window_tokens,
            max_output_tokens: profile.max_output_tokens,
            tokenizer: profile.tokenizer.clone(),
            headers: profile.headers.clone(),
        }
    }

    /// Return the normalized protocol used to choose the concrete LLM transport implementation.
    pub fn effective_protocol(&self) -> &'static str {
        normalize_llm_protocol(&self.protocol)
    }

    /// Return the effective context window tokens for the resolved provider.
    pub fn context_window_tokens(&self) -> usize {
        effective_context_window_tokens(&self.model, self.context_window_tokens)
    }

    /// Return the effective max output tokens for the resolved provider.
    pub fn max_output_tokens(&self) -> usize {
        effective_max_output_tokens(&self.model, self.max_output_tokens)
    }

    /// Return whether the resolved provider adds static request headers.
    pub fn has_headers(&self) -> bool {
        !self.headers.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct LLMConfig {
    pub protocol: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub api_key_path: PathBuf,
    pub mock_response: String,
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub tokenizer: String,
    pub system_prompt: String,
    pub active_provider: Option<String>,
    pub providers: HashMap<String, LLMProviderProfileConfig>,
    #[doc(hidden)]
    pub legacy_fields_explicit: bool,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            protocol: default_runtime_llm_protocol(),
            provider: default_llm_provider(),
            model: default_llm_model(),
            base_url: default_llm_base_url(),
            api_key: default_llm_api_key(),
            api_key_path: default_llm_api_key_path(),
            mock_response: default_llm_mock_response(),
            context_window_tokens: default_llm_context_window_tokens(),
            max_output_tokens: default_llm_max_output_tokens(),
            tokenizer: default_llm_tokenizer(),
            system_prompt: default_llm_system_prompt(),
            active_provider: None,
            providers: HashMap::new(),
            legacy_fields_explicit: false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawLLMConfig {
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_path: Option<PathBuf>,
    #[serde(default)]
    mock_response: Option<String>,
    #[serde(default)]
    context_window_tokens: Option<usize>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    tokenizer: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    active_provider: Option<String>,
    #[serde(default)]
    providers: RawLLMProviders,
}

impl<'de> Deserialize<'de> for LLMConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawLLMConfig::deserialize(deserializer)?;
        let provider_defaults = RawLLMProviderDefaults::from_raw_llm_config(&raw);
        let (providers, providers_from_list) = raw
            .providers
            .into_profiles(&provider_defaults)
            .map_err(serde::de::Error::custom)?;
        let active_provider = if providers_from_list {
            match (
                raw.active_provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
                raw.provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
            ) {
                (Some(active), Some(provider)) if active != provider => {
                    return Err(serde::de::Error::custom(format!(
                        "llm.active_provider `{active}` conflicts with llm.provider `{provider}`"
                    )));
                }
                (Some(active), _) => Some(active.to_string()),
                (None, Some(provider)) => Some(provider.to_string()),
                (None, None) => None,
            }
        } else {
            raw.active_provider.clone()
        };

        let legacy_fields_explicit = if providers_from_list {
            false
        } else {
            raw.protocol.is_some()
                || raw.provider.is_some()
                || raw.model.is_some()
                || raw.base_url.is_some()
                || raw.api_key.is_some()
                || raw.api_key_path.is_some()
                || raw.mock_response.is_some()
                || raw.context_window_tokens.is_some()
                || raw.max_output_tokens.is_some()
                || raw.tokenizer.is_some()
        };

        Ok(Self {
            protocol: if providers_from_list {
                default_deserialized_llm_protocol()
            } else {
                raw.protocol
                    .unwrap_or_else(default_deserialized_llm_protocol)
            },
            provider: if providers_from_list {
                default_llm_provider()
            } else {
                raw.provider.unwrap_or_else(default_llm_provider)
            },
            model: if providers_from_list {
                default_llm_model()
            } else {
                raw.model.unwrap_or_else(default_llm_model)
            },
            base_url: if providers_from_list {
                default_llm_base_url()
            } else {
                raw.base_url.unwrap_or_else(default_llm_base_url)
            },
            api_key: if providers_from_list {
                default_llm_api_key()
            } else {
                raw.api_key.unwrap_or_else(default_llm_api_key)
            },
            api_key_path: if providers_from_list {
                default_llm_api_key_path()
            } else {
                raw.api_key_path.unwrap_or_else(default_llm_api_key_path)
            },
            mock_response: if providers_from_list {
                default_llm_mock_response()
            } else {
                raw.mock_response.unwrap_or_else(default_llm_mock_response)
            },
            context_window_tokens: if providers_from_list {
                default_llm_context_window_tokens()
            } else {
                raw.context_window_tokens
            },
            max_output_tokens: if providers_from_list {
                default_llm_max_output_tokens()
            } else {
                raw.max_output_tokens
            },
            tokenizer: if providers_from_list {
                default_llm_tokenizer()
            } else {
                raw.tokenizer.unwrap_or_else(default_llm_tokenizer)
            },
            system_prompt: raw.system_prompt.unwrap_or_else(default_llm_system_prompt),
            active_provider,
            providers,
            legacy_fields_explicit,
        })
    }
}

impl LLMConfig {
    /// Return the normalized protocol used to choose the concrete LLM transport implementation.
    ///
    /// 当前只接受 `llm.protocol`，用于决定具体走哪条 LLM 传输实现；未知协议会返回
    /// `unknown`，并在配置校验或 provider 构建阶段被拒绝。
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMConfig;
    ///
    /// let config = LLMConfig {
    ///     protocol: "openai".to_string(),
    ///     ..LLMConfig::default()
    /// };
    ///
    /// assert_eq!(config.effective_protocol(), "openai_compatible");
    /// ```
    pub fn effective_protocol(&self) -> &'static str {
        self.resolve_active_provider()
            .map(|provider| provider.effective_protocol())
            .unwrap_or_else(|_| normalize_llm_protocol(&self.protocol))
    }

    /// Return the effective context window tokens for the configured model.
    ///
    /// 显式配置优先；如果用户未填写，则尝试按已知模型规格兜底；仍无法识别时回落到通用默认值。
    pub fn context_window_tokens(&self) -> usize {
        self.resolve_active_provider()
            .map(|provider| provider.context_window_tokens())
            .unwrap_or_else(|_| {
                effective_context_window_tokens(&self.model, self.context_window_tokens)
            })
    }

    /// Return the effective max output tokens for the configured model.
    ///
    /// 显式配置优先；如果用户未填写，则尝试按已知模型规格兜底；仍无法识别时回落到通用默认值。
    pub fn max_output_tokens(&self) -> usize {
        self.resolve_active_provider()
            .map(|provider| provider.max_output_tokens())
            .unwrap_or_else(|_| effective_max_output_tokens(&self.model, self.max_output_tokens))
    }

    /// Return the tokenizer configured for the active provider view.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMConfig;
    ///
    /// let config = LLMConfig::default();
    ///
    /// assert_eq!(config.effective_tokenizer(), "chars_div4".to_string());
    /// ```
    pub fn effective_tokenizer(&self) -> String {
        if let Ok(provider) = self.resolve_active_provider()
            && !provider.tokenizer.is_empty()
        {
            return provider.tokenizer;
        }

        self.tokenizer.clone()
    }

    /// Return the effective assistant system prompt configured for worker initialization.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMConfig;
    ///
    /// let config = LLMConfig::default();
    ///
    /// assert!(!config.effective_system_prompt().trim().is_empty());
    /// ```
    pub fn effective_system_prompt(&self) -> &str {
        let prompt = self.system_prompt.trim();
        if prompt.is_empty() {
            DEFAULT_ASSISTANT_SYSTEM_PROMPT
        } else {
            prompt
        }
    }

    /// Resolve the active provider into one stable runtime view.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMConfig;
    ///
    /// let config = LLMConfig::default();
    /// let resolved = config
    ///     .resolve_active_provider()
    ///     .expect("default config should resolve");
    ///
    /// assert_eq!(resolved.name, "unknown");
    /// assert_eq!(resolved.effective_protocol(), "mock");
    /// ```
    pub fn resolve_active_provider(&self) -> Result<ResolvedLLMProviderConfig> {
        if self.uses_provider_profiles() {
            let active_provider = self
                .active_provider
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .context("llm.active_provider is required when llm.providers is configured")?;
            let profile = self.providers.get(active_provider).with_context(|| {
                format!("llm.active_provider `{active_provider}` does not exist in llm.providers")
            })?;
            return Ok(ResolvedLLMProviderConfig::from_profile(
                active_provider,
                profile,
            ));
        }

        Ok(ResolvedLLMProviderConfig {
            name: self.provider.clone(),
            protocol: self.protocol.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            api_key_path: self.api_key_path.clone(),
            mock_response: self.mock_response.clone(),
            context_window_tokens: self.context_window_tokens,
            max_output_tokens: self.max_output_tokens,
            tokenizer: self.tokenizer.clone(),
            headers: HashMap::new(),
        })
    }

    /// Resolve every declared provider profile into a stable map keyed by provider name.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::config::LLMConfig;
    ///
    /// let config = LLMConfig::default();
    /// let providers = config
    ///     .resolve_all_providers()
    ///     .expect("default config should resolve");
    ///
    /// assert_eq!(providers["unknown"].effective_protocol(), "mock");
    /// ```
    pub fn resolve_all_providers(&self) -> Result<HashMap<String, ResolvedLLMProviderConfig>> {
        if self.uses_provider_profiles() {
            let mut resolved = HashMap::with_capacity(self.providers.len());
            for (name, profile) in &self.providers {
                resolved.insert(
                    name.clone(),
                    ResolvedLLMProviderConfig::from_profile(name, profile),
                );
            }
            return Ok(resolved);
        }

        Ok(HashMap::from([(
            self.provider.clone(),
            self.resolve_active_provider()?,
        )]))
    }

    fn validate(&self) -> Result<()> {
        if self.uses_provider_profiles() {
            if self.providers.is_empty() {
                bail!("llm.providers must not be empty when llm.active_provider is configured");
            }
            if self.has_legacy_fields_for_multi_provider_mode() {
                bail!(
                    "llm legacy single-provider fields and llm.active_provider/llm.providers cannot be configured together"
                );
            }
            let active_provider = self
                .active_provider
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .context("llm.active_provider is required when llm.providers is configured")?;
            let Some(profile) = self.providers.get(active_provider) else {
                bail!("llm.active_provider `{active_provider}` does not exist in llm.providers");
            };
            for (name, profile) in &self.providers {
                profile.validate(&format!("llm.providers.{name}"))?;
            }
            profile.validate(&format!("llm.providers.{active_provider}"))?;
            return Ok(());
        }

        validate_llm_profile(
            &self.protocol,
            &self.model,
            self.context_window_tokens,
            self.max_output_tokens,
            &self.tokenizer,
            &HashMap::new(),
            "llm",
        )
    }

    fn uses_provider_profiles(&self) -> bool {
        self.active_provider.is_some() || !self.providers.is_empty()
    }

    fn has_legacy_fields_for_multi_provider_mode(&self) -> bool {
        self.legacy_fields_explicit || !self.matches_runtime_legacy_defaults()
    }

    fn matches_runtime_legacy_defaults(&self) -> bool {
        (self.protocol.is_empty() || self.protocol == default_runtime_llm_protocol())
            && self.provider == default_llm_provider()
            && self.model == default_llm_model()
            && self.base_url == default_llm_base_url()
            && self.api_key == default_llm_api_key()
            && self.api_key_path == default_llm_api_key_path()
            && self.mock_response == default_llm_mock_response()
            && self.context_window_tokens == default_llm_context_window_tokens()
            && self.max_output_tokens == default_llm_max_output_tokens()
            && self.tokenizer == default_llm_tokenizer()
    }
}

fn normalize_llm_protocol(protocol: &str) -> &'static str {
    let normalized_protocol = protocol.trim().to_ascii_lowercase();
    match normalized_protocol.as_str() {
        "mock" | "mock_llm" => "mock",
        "openai" | "openai_compatible" | "openai_chat_completion" | "openai_chat_completions" => {
            "openai_compatible"
        }
        "openai-compatible" => "openai_compatible",
        "openai_response" | "openai_responses" | "responses" => "openai_responses",
        "openai-response" | "openai-responses" => "openai_responses",
        "anthropic" | "claude" => "anthropic",
        _ => "unknown",
    }
}

fn effective_context_window_tokens(model: &str, configured: Option<usize>) -> usize {
    configured
        .or_else(|| known_model_token_limits(model).map(|limits| limits.context_window_tokens))
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
}

fn effective_max_output_tokens(model: &str, configured: Option<usize>) -> usize {
    configured
        .or_else(|| known_model_token_limits(model).map(|limits| limits.max_output_tokens))
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
}

fn validate_llm_profile(
    protocol: &str,
    model: &str,
    context_window_tokens: Option<usize>,
    max_output_tokens: Option<usize>,
    tokenizer: &str,
    headers: &HashMap<String, String>,
    field_prefix: &str,
) -> Result<()> {
    if protocol.trim().is_empty() {
        bail!("{field_prefix}.protocol is required");
    }
    if matches!(normalize_llm_protocol(protocol), "unknown") {
        bail!(
            "{field_prefix}.protocol `{}` is not supported",
            protocol.trim()
        );
    }
    if context_window_tokens.is_some_and(|value| value == 0) {
        bail!("{field_prefix}.context_window_tokens must be greater than 0");
    }
    if max_output_tokens.is_some_and(|value| value == 0) {
        bail!("{field_prefix}.max_output_tokens must be greater than 0");
    }
    if effective_max_output_tokens(model, max_output_tokens)
        > effective_context_window_tokens(model, context_window_tokens)
    {
        bail!(
            "{field_prefix}.max_output_tokens must be less than or equal to {field_prefix}.context_window_tokens"
        );
    }
    if tokenizer.trim().is_empty() {
        bail!("{field_prefix}.tokenizer must not be blank");
    }
    if !matches!(tokenizer.trim(), "chars_div4") {
        bail!(
            "{field_prefix}.tokenizer `{}` is not supported yet; expected `chars_div4`",
            tokenizer
        );
    }
    for header_name in headers.keys() {
        if header_name.trim().is_empty() {
            bail!("{field_prefix}.headers contains a blank header name");
        }
    }

    Ok(())
}

fn known_model_token_limits(model: &str) -> Option<ModelTokenLimits> {
    let normalized_model = model.trim().to_ascii_lowercase();
    match normalized_model.as_str() {
        "kimi-k2.5"
        | "kimi-k2-0905-preview"
        | "kimi-k2-turbo-preview"
        | "kimi-k2-thinking"
        | "kimi-k2-thinking-turbo" => Some(ModelTokenLimits {
            context_window_tokens: KIMI_K2_5_CONTEXT_WINDOW_TOKENS,
            max_output_tokens: KIMI_K2_5_MAX_OUTPUT_TOKENS,
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct ModelTokenLimits {
    context_window_tokens: usize,
    max_output_tokens: usize,
}

fn validate_ratio(value: f64, field_name: &str) -> Result<()> {
    if !(0.0..=1.0).contains(&value) || value == 0.0 {
        bail!("{field_name} must be greater than 0 and less than or equal to 1");
    }

    Ok(())
}
