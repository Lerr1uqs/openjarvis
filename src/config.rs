//! Configuration loading and default values for the application, channels, and LLM provider.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str = "你是 OpenJarvis，一个有帮助、可靠、简洁的 AI 助手。请直接回答用户问题；如需要工具，基于上下文发起工具调用。";

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
    pub fn load() -> Result<Self> {
        let path = env::var("OPENJARVIS_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        Self::from_path(path)
    }

    /// Load configuration from a specific YAML path, falling back to defaults when the file is missing.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config = serde_yaml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
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

    fn validate(&self) -> Result<()> {
        self.agent.validate()
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

    pub(crate) fn validate(&self) -> Result<()> {
        self.hook.validate()
    }
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
