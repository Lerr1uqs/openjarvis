//! Hook event types and registry used to observe the agent loop lifecycle.

use crate::config::AgentHookConfig;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;
use std::{process::Stdio, sync::Arc};
use tokio::sync::RwLock;
use tokio::{
    process::Command,
    time::{Duration, timeout},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEventKind {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    Stop,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PermissionRequest,
    Notification,
    SessionStart,
    SessionEnd,
    Setup,
    TeammateIdle,
    TaskCompleted,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
}

impl HookEventKind {
    /// Return the stable snake_case event name used in config keys and hook env vars.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::HookEventKind;
    ///
    /// assert_eq!(HookEventKind::PreToolUse.as_str(), "pre_tool_use");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::PostToolUseFailure => "post_tool_use_failure",
            Self::UserPromptSubmit => "user_prompt_submit",
            Self::Stop => "stop",
            Self::SubagentStart => "subagent_start",
            Self::SubagentStop => "subagent_stop",
            Self::PreCompact => "pre_compact",
            Self::PermissionRequest => "permission_request",
            Self::Notification => "notification",
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
            Self::Setup => "setup",
            Self::TeammateIdle => "teammate_idle",
            Self::TaskCompleted => "task_completed",
            Self::ConfigChange => "config_change",
            Self::WorktreeCreate => "worktree_create",
            Self::WorktreeRemove => "worktree_remove",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub kind: HookEventKind,
    pub payload: Value,
}

#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Return the stable handler name used for logs and diagnostics.
    fn name(&self) -> &'static str;

    /// Handle one hook event emitted by the agent loop.
    async fn handle(&self, event: &HookEvent) -> Result<()>;
}

#[derive(Default)]
pub struct HookRegistry {
    handlers: RwLock<Vec<Arc<dyn HookHandler>>>,
}

impl HookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a hook registry from the configured `agent.hook` section.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::HookRegistry, config::AppConfig};
    ///
    /// let config = AppConfig::default();
    /// let registry = HookRegistry::from_config(config.agent_config().hook_config()).await?;
    /// assert_eq!(registry.len().await, 0);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &AgentHookConfig) -> Result<Self> {
        config.validate()?;
        let registry = Self::new();
        registry.register_configured(config).await?;
        Ok(registry)
    }

    /// Register one hook handler.
    pub async fn register(&self, handler: Arc<dyn HookHandler>) {
        self.handlers.write().await.push(handler);
    }

    /// Register all configured hook scripts from `agent.hook`.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::HookRegistry, config::AppConfig};
    ///
    /// let config = AppConfig::default();
    /// let registry = HookRegistry::new();
    /// registry
    ///     .register_configured(config.agent_config().hook_config())
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn register_configured(&self, config: &AgentHookConfig) -> Result<()> {
        config.validate()?;
        for (event_name, command) in config.configured_commands() {
            self.register(Arc::new(ConfiguredCommandHook::new(
                hook_event_kind_from_name(event_name)?,
                command.parts().to_vec(),
            )))
            .await;
        }

        Ok(())
    }

    /// Emit an event to all registered handlers in registration order.
    pub async fn emit(&self, event: HookEvent) -> Result<()> {
        let handlers = self
            .handlers
            .read()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for handler in handlers {
            handler.handle(&event).await?;
        }

        Ok(())
    }

    /// Return the number of registered handlers.
    pub async fn len(&self) -> usize {
        self.handlers.read().await.len()
    }
}

const DEFAULT_HOOK_TIMEOUT_MS: u64 = 30_000;

struct ConfiguredCommandHook {
    kind: HookEventKind,
    command: Vec<String>,
}

impl ConfiguredCommandHook {
    fn new(kind: HookEventKind, command: Vec<String>) -> Self {
        Self { kind, command }
    }

    fn handler_name(kind: &HookEventKind) -> &'static str {
        match kind {
            HookEventKind::PreToolUse => "configured_command_hook_pre_tool_use",
            HookEventKind::PostToolUse => "configured_command_hook_post_tool_use",
            HookEventKind::PostToolUseFailure => "configured_command_hook_post_tool_use_failure",
            HookEventKind::UserPromptSubmit => "configured_command_hook_user_prompt_submit",
            HookEventKind::Stop => "configured_command_hook_stop",
            HookEventKind::SubagentStart => "configured_command_hook_subagent_start",
            HookEventKind::SubagentStop => "configured_command_hook_subagent_stop",
            HookEventKind::PreCompact => "configured_command_hook_pre_compact",
            HookEventKind::PermissionRequest => "configured_command_hook_permission_request",
            HookEventKind::Notification => "configured_command_hook_notification",
            HookEventKind::SessionStart => "configured_command_hook_session_start",
            HookEventKind::SessionEnd => "configured_command_hook_session_end",
            HookEventKind::Setup => "configured_command_hook_setup",
            HookEventKind::TeammateIdle => "configured_command_hook_teammate_idle",
            HookEventKind::TaskCompleted => "configured_command_hook_task_completed",
            HookEventKind::ConfigChange => "configured_command_hook_config_change",
            HookEventKind::WorktreeCreate => "configured_command_hook_worktree_create",
            HookEventKind::WorktreeRemove => "configured_command_hook_worktree_remove",
        }
    }

    fn build_process(&self) -> Command {
        #[cfg(windows)]
        {
            let mut process = Command::new("powershell");
            process
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(build_windows_hook_command(&self.command));
            process
        }

        #[cfg(not(windows))]
        {
            let mut process = Command::new("sh");
            process
                .arg("-lc")
                .arg(build_posix_hook_command(&self.command));
            process
        }
    }
}

#[async_trait]
impl HookHandler for ConfiguredCommandHook {
    fn name(&self) -> &'static str {
        Self::handler_name(&self.kind)
    }

    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if event.kind != self.kind {
            return Ok(());
        }

        let mut process = self.build_process();
        process
            .stdin(Stdio::null())
            .env("OPENJARVIS_HOOK_EVENT", event.kind.as_str())
            .env("OPENJARVIS_HOOK_PAYLOAD", event.payload.to_string());

        let output = match timeout(
            Duration::from_millis(DEFAULT_HOOK_TIMEOUT_MS),
            process.output(),
        )
        .await
        {
            Ok(result) => result.with_context(|| {
                format!(
                    "failed to execute hook {} with command {:?}",
                    event.kind.as_str(),
                    self.command
                )
            })?,
            Err(_) => {
                bail!(
                    "hook {} timed out after {} ms",
                    event.kind.as_str(),
                    DEFAULT_HOOK_TIMEOUT_MS
                );
            }
        };

        if output.status.success() {
            return Ok(());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };

        bail!(
            "hook {} failed with status {:?}: {}",
            event.kind.as_str(),
            output.status.code(),
            detail
        )
    }
}

fn hook_event_kind_from_name(event_name: &str) -> Result<HookEventKind> {
    match event_name {
        "pre_tool_use" => Ok(HookEventKind::PreToolUse),
        "post_tool_use" => Ok(HookEventKind::PostToolUse),
        "post_tool_use_failure" => Ok(HookEventKind::PostToolUseFailure),
        "user_prompt_submit" => Ok(HookEventKind::UserPromptSubmit),
        "stop" => Ok(HookEventKind::Stop),
        "subagent_start" => Ok(HookEventKind::SubagentStart),
        "subagent_stop" => Ok(HookEventKind::SubagentStop),
        "pre_compact" => Ok(HookEventKind::PreCompact),
        "permission_request" => Ok(HookEventKind::PermissionRequest),
        "notification" => Ok(HookEventKind::Notification),
        "session_start" => Ok(HookEventKind::SessionStart),
        "session_end" => Ok(HookEventKind::SessionEnd),
        "setup" => Ok(HookEventKind::Setup),
        "teammate_idle" => Ok(HookEventKind::TeammateIdle),
        "task_completed" => Ok(HookEventKind::TaskCompleted),
        "config_change" => Ok(HookEventKind::ConfigChange),
        "worktree_create" => Ok(HookEventKind::WorktreeCreate),
        "worktree_remove" => Ok(HookEventKind::WorktreeRemove),
        other => bail!("unsupported hook event name: {other}"),
    }
}

#[cfg(windows)]
fn build_windows_hook_command(parts: &[String]) -> String {
    let quoted = parts
        .iter()
        .map(|part| format!("'{}'", part.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(" ");
    format!("& {quoted}")
}

#[cfg(not(windows))]
fn build_posix_hook_command(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| format!("'{}'", part.replace('\'', "'\"'\"'")))
        .collect::<Vec<_>>()
        .join(" ")
}
