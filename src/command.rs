//! Slash command parsing and dispatch before router messages enter session or agent flows.
//!
//! The router uses this module as a hard pre-processing stage. Any incoming message that starts
//! with `/` is treated as a command message. Every command is thread-scoped, so the router must
//! resolve the target `Thread` before execution. Unknown commands return a formatted failure
//! response without touching `AgentWorker`.
//! Some Feishu channel messages may arrive as `@_user_1 /echo zxf`, so command matching strips
//! one leading Feishu mention token before checking whether the message starts with `/`.
//! Commands can still mutate resolved thread state even though command messages themselves do not
//! enter the persisted session history.

use crate::{
    agent::ToolRegistry,
    compact::{ContextBudgetEstimator, ContextBudgetReport},
    config::{AppConfig, try_global_config},
    context::{ChatMessage, ChatMessageRole, ContextTokenKind},
    model::IncomingMessage,
    thread::{Thread, ThreadRuntime},
};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use std::{collections::HashMap, path::Path, sync::Arc};
use tracing::info;

const CONTEXT_MESSAGE_PREVIEW_LIMIT: usize = 48;

/// Formatted command execution reply returned back to the upstream channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReply {
    name: String,
    success: bool,
    message: String,
}

impl CommandReply {
    /// Create one successful command reply.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandReply;
    ///
    /// let reply = CommandReply::success("echo", "hello");
    /// assert_eq!(reply.formatted_content(), "[Command][echo][SUCCESS]: hello");
    /// ```
    pub fn success(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: true,
            message: message.into(),
        }
    }

    /// Create one failed command reply.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandReply;
    ///
    /// let reply = CommandReply::failed("equal", "arguments must match");
    /// assert_eq!(
    ///     reply.formatted_content(),
    ///     "[Command][equal][FAILED]: arguments must match"
    /// );
    /// ```
    pub fn failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: false,
            message: message.into(),
        }
    }

    /// Return the normalized command name.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandReply;
    ///
    /// let reply = CommandReply::success("test", "ok");
    /// assert_eq!(reply.name(), "test");
    /// ```
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return whether the command succeeded.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandReply;
    ///
    /// assert!(CommandReply::success("test", "ok").is_success());
    /// assert!(!CommandReply::failed("test", "no").is_success());
    /// ```
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// Return the formatted user-facing command result text.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandReply;
    ///
    /// let reply = CommandReply::success("echo", "hello world");
    /// assert_eq!(reply.formatted_content(), "[Command][echo][SUCCESS]: hello world");
    /// ```
    pub fn formatted_content(&self) -> String {
        let status = if self.success { "SUCCESS" } else { "FAILED" };
        format!("[Command][{}][{}]: {}", self.name, status, self.message)
    }
}

/// One parsed slash command invocation extracted from an incoming message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    name: String,
    raw_arguments: String,
    arguments: Vec<String>,
}

impl CommandInvocation {
    /// Parse one incoming message into a command invocation when it starts with `/`.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandInvocation;
    ///
    /// let parsed = CommandInvocation::parse("/equal left right")
    ///     .expect("parse should succeed")
    ///     .expect("message should be treated as a command");
    ///
    /// assert_eq!(parsed.name(), "equal");
    /// assert_eq!(parsed.arguments(), ["left", "right"]);
    /// ```
    pub fn parse(message: &str) -> Result<Option<Self>> {
        let trimmed = message.trim_start();
        if !trimmed.starts_with('/') {
            return Ok(None);
        }

        let body = &trimmed[1..];
        if body.trim().is_empty() {
            return Err(anyhow!("command name is required"));
        }

        let command_name_end = body
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(body.len());
        let name = body[..command_name_end].trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(anyhow!("command name is required"));
        }

        let raw_arguments = body[command_name_end..].trim_start().to_string();
        let arguments = if raw_arguments.is_empty() {
            Vec::new()
        } else {
            raw_arguments
                .split_whitespace()
                .map(ToString::to_string)
                .collect()
        };

        Ok(Some(Self {
            name,
            raw_arguments,
            arguments,
        }))
    }

    /// Return the normalized command name.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandInvocation;
    ///
    /// let parsed = CommandInvocation::parse("/test")
    ///     .expect("parse should succeed")
    ///     .expect("message should be a command");
    /// assert_eq!(parsed.name(), "test");
    /// ```
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the raw argument string after the command name.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandInvocation;
    ///
    /// let parsed = CommandInvocation::parse("/echo hello world")
    ///     .expect("parse should succeed")
    ///     .expect("message should be a command");
    /// assert_eq!(parsed.raw_arguments(), "hello world");
    /// ```
    pub fn raw_arguments(&self) -> &str {
        &self.raw_arguments
    }

    /// Return the whitespace-tokenized command arguments.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandInvocation;
    ///
    /// let parsed = CommandInvocation::parse("/equal left right")
    ///     .expect("parse should succeed")
    ///     .expect("message should be a command");
    /// assert_eq!(parsed.arguments(), ["left", "right"]);
    /// ```
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

/// Async command handler abstraction used by the router-facing command registry.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Return whether this command may only run after the active agent turn finishes.
    fn requires_idle_thread(&self) -> bool {
        false
    }

    /// Execute one parsed command invocation and return a formatted reply payload.
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
        thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply>;
}

/// Slash command registry used as a pre-routing filter.
pub struct CommandRegistry {
    handlers: HashMap<String, Arc<dyn CommandHandler>>,
}

impl CommandRegistry {
    /// Create an empty command registry.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandRegistry;
    ///
    /// let registry = CommandRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Create a registry with the built-in framework smoke-test commands.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandRegistry;
    ///
    /// let registry = CommandRegistry::with_builtin_commands();
    /// assert!(!registry.is_empty());
    /// ```
    pub fn with_builtin_commands() -> Self {
        let mut registry = Self::new();
        registry.register_standard_builtin_commands();
        registry
    }

    /// Create a registry with the standard built-in commands plus tool-aware browser commands.
    pub fn with_builtin_commands_and_tools(tools: Arc<ToolRegistry>) -> Self {
        let mut registry = Self::with_builtin_commands();
        registry
            .register(
                "browser-export-cookies",
                Arc::new(BrowserExportCookiesCommand::new(tools)),
            )
            .expect("built-in browser-export-cookies command should register");
        registry
    }

    /// Return whether the registry currently has no handlers.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::command::CommandRegistry;
    ///
    /// assert!(CommandRegistry::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    fn register_standard_builtin_commands(&mut self) {
        self.register("test", Arc::new(TestCommand))
            .expect("built-in test command should register");
        self.register("equal", Arc::new(EqualCommand))
            .expect("built-in equal command should register");
        self.register("echo", Arc::new(EchoCommand))
            .expect("built-in echo command should register");
        self.register("context", Arc::new(ContextCommand))
            .expect("built-in context command should register");
        self.register("new", Arc::new(NewCommand))
            .expect("built-in new command should register");
    }

    /// Register one command handler under a normalized slash command name.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use async_trait::async_trait;
    /// use openjarvis::command::{CommandHandler, CommandInvocation, CommandRegistry, CommandReply};
    /// use openjarvis::model::IncomingMessage;
    /// use std::sync::Arc;
    ///
    /// struct PingCommand;
    ///
    /// #[async_trait]
    /// impl CommandHandler for PingCommand {
    ///     async fn execute(
    ///         &self,
    ///         _invocation: &CommandInvocation,
    ///         _incoming: &IncomingMessage,
    ///         _thread_context: &mut openjarvis::thread::Thread,
    ///         _thread_runtime: Option<&openjarvis::thread::ThreadRuntime>,
    ///     ) -> anyhow::Result<CommandReply> {
    ///         Ok(CommandReply::success("ping", "pong"))
    ///     }
    /// }
    ///
    /// let mut registry = CommandRegistry::new();
    /// registry
    ///     .register("ping", Arc::new(PingCommand))
    ///     .expect("ping command should register");
    /// ```
    pub fn register(
        &mut self,
        name: impl AsRef<str>,
        handler: Arc<dyn CommandHandler>,
    ) -> Result<()> {
        let name = normalize_command_name(name.as_ref())?;
        if self.handlers.insert(name.clone(), handler).is_some() {
            bail!("command `{name}` is already registered");
        }
        Ok(())
    }

    /// Return whether one incoming message should be treated as a slash command.
    pub fn is_command(&self, incoming: &IncomingMessage) -> Result<bool> {
        let normalized_content = remove_prefix_at_if_exist(incoming);
        Ok(CommandInvocation::parse(&normalized_content)?.is_some())
    }

    /// Return the command reply that should be sent when the target thread is still running.
    pub fn running_thread_reply(&self, incoming: &IncomingMessage) -> Result<Option<CommandReply>> {
        let normalized_content = remove_prefix_at_if_exist(incoming);
        let Some(invocation) = CommandInvocation::parse(&normalized_content)? else {
            return Ok(None);
        };
        let Some(handler) = self.handlers.get(invocation.name()) else {
            return Ok(None);
        };
        if !handler.requires_idle_thread() {
            return Ok(None);
        }

        Ok(Some(CommandReply::failed(
            invocation.name(),
            format!(
                "current thread is running; /{} is unavailable until the active agent turn completes",
                invocation.name()
            ),
        )))
    }

    /// Try to execute one incoming message as a slash command with the resolved target thread context.
    pub async fn try_execute_with_thread_context(
        &self,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
    ) -> Result<Option<CommandReply>> {
        self.try_execute_with_thread_context_and_runtime(incoming, thread_context, None)
            .await
    }

    /// Try to execute one incoming message as a slash command with one optional installed runtime.
    pub async fn try_execute_with_thread_context_and_runtime(
        &self,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
        thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<Option<CommandReply>> {
        let normalized_content = remove_prefix_at_if_exist(incoming);
        let Some(invocation) = CommandInvocation::parse(&normalized_content)? else {
            return Ok(None);
        };

        let Some(handler) = self.handlers.get(invocation.name()) else {
            return Ok(Some(CommandReply::failed(
                invocation.name(),
                "unknown command",
            )));
        };

        Ok(Some(
            handler
                .execute(&invocation, incoming, thread_context, thread_runtime)
                .await?,
        ))
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::with_builtin_commands()
    }
}

struct TestCommand;

#[async_trait]
impl CommandHandler for TestCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _incoming: &IncomingMessage,
        _thread_context: &mut Thread,
        _thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        let message = if invocation.raw_arguments().is_empty() {
            "test command ok".to_string()
        } else {
            invocation.raw_arguments().to_string()
        };
        Ok(CommandReply::success(invocation.name(), message))
    }
}

struct EqualCommand;

#[async_trait]
impl CommandHandler for EqualCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _incoming: &IncomingMessage,
        _thread_context: &mut Thread,
        _thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        let arguments = invocation.arguments();
        if arguments.len() != 2 {
            return Ok(CommandReply::failed(
                invocation.name(),
                "equal expects exactly 2 arguments",
            ));
        }

        if arguments[0] == arguments[1] {
            Ok(CommandReply::success(
                invocation.name(),
                format!("{} == {}", arguments[0], arguments[1]),
            ))
        } else {
            Ok(CommandReply::failed(
                invocation.name(),
                format!("{} != {}", arguments[0], arguments[1]),
            ))
        }
    }
}

struct EchoCommand;

#[async_trait]
impl CommandHandler for EchoCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _incoming: &IncomingMessage,
        _thread_context: &mut Thread,
        _thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        Ok(CommandReply::success(
            invocation.name(),
            invocation.raw_arguments().to_string(),
        ))
    }
}

struct ContextCommand;

impl ContextCommand {
    fn usage(name: &str) -> CommandReply {
        CommandReply::failed(name, "usage: /context [role|detail [count]]")
    }
}

#[async_trait]
impl CommandHandler for ContextCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _incoming: &IncomingMessage,
        thread_context: &mut Thread,
        _thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        let messages = thread_context.messages();
        let estimator = context_budget_estimator();

        match invocation.arguments() {
            [] => {
                let report = estimator.estimate(&messages, &[]);
                info!(
                    thread_id = %thread_context.locator.thread_id,
                    external_thread_id = %thread_context.locator.external_thread_id,
                    mode = "summary",
                    message_count = messages.len(),
                    total_estimated_tokens = report.total_estimated_tokens,
                    utilization_ratio = report.utilization_ratio,
                    "inspected current thread context usage by command"
                );
                Ok(CommandReply::success(
                    invocation.name(),
                    format_context_summary(thread_context, messages.len(), &report),
                ))
            }
            [mode] if mode.eq_ignore_ascii_case("role") => {
                let report = estimator.estimate(&messages, &[]);
                info!(
                    thread_id = %thread_context.locator.thread_id,
                    external_thread_id = %thread_context.locator.external_thread_id,
                    mode = "role",
                    message_count = messages.len(),
                    context_window_tokens = estimator.context_window_tokens(),
                    total_estimated_tokens = report.total_estimated_tokens,
                    "inspected per-role thread context usage by command"
                );
                Ok(CommandReply::success(
                    invocation.name(),
                    format_context_role_report(thread_context, &messages, &estimator, &report),
                ))
            }
            [mode] if mode.eq_ignore_ascii_case("detail") => {
                info!(
                    thread_id = %thread_context.locator.thread_id,
                    external_thread_id = %thread_context.locator.external_thread_id,
                    mode = "detail",
                    message_count = messages.len(),
                    context_window_tokens = estimator.context_window_tokens(),
                    detail_count = 20,
                    "inspected per-message thread context usage by command"
                );
                Ok(CommandReply::success(
                    invocation.name(),
                    format_context_detail_report(thread_context, &messages, &estimator, 20),
                ))
            }
            [mode, count] if mode.eq_ignore_ascii_case("detail") => {
                let Ok(detail_count) = count.parse::<usize>() else {
                    return Ok(Self::usage(invocation.name()));
                };
                info!(
                    thread_id = %thread_context.locator.thread_id,
                    external_thread_id = %thread_context.locator.external_thread_id,
                    mode = "detail",
                    message_count = messages.len(),
                    context_window_tokens = estimator.context_window_tokens(),
                    detail_count,
                    "inspected per-message thread context usage by command"
                );
                Ok(CommandReply::success(
                    invocation.name(),
                    format_context_detail_report(
                        thread_context,
                        &messages,
                        &estimator,
                        detail_count,
                    ),
                ))
            }
            _ => Ok(Self::usage(invocation.name())),
        }
    }
}

struct NewCommand;

impl NewCommand {
    fn usage(name: &str) -> CommandReply {
        CommandReply::failed(name, "usage: /new")
    }
}

#[async_trait]
impl CommandHandler for NewCommand {
    fn requires_idle_thread(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        invocation: &CommandInvocation,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
        thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        if !invocation.arguments().is_empty() {
            return Ok(Self::usage(invocation.name()));
        }

        let Some(thread_runtime) = thread_runtime else {
            return Ok(CommandReply::failed(
                invocation.name(),
                "current process does not have one installed thread runtime; /new is unavailable",
            ));
        };

        info!(
            thread_id = %thread_context.locator.thread_id,
            external_thread_id = %thread_context.locator.external_thread_id,
            thread_agent_kind = thread_context.thread_agent_kind().as_str(),
            child_thread = thread_context.child_thread_identity().is_some(),
            "reinitializing current thread by command"
        );
        thread_runtime
            .reinitialize_thread(thread_context, incoming.received_at)
            .await?;
        Ok(CommandReply::success(
            invocation.name(),
            format!(
                "reinitialized current thread `{}`; stable system prefix and thread-scoped runtime state have been rebuilt",
                thread_context.locator.external_thread_id
            ),
        ))
    }
}

struct BrowserExportCookiesCommand {
    tools: Arc<ToolRegistry>,
}

impl BrowserExportCookiesCommand {
    fn new(tools: Arc<ToolRegistry>) -> Self {
        Self { tools }
    }

    fn usage(name: &str) -> CommandReply {
        CommandReply::failed(name, "usage: /browser-export-cookies <path>")
    }
}

#[async_trait]
impl CommandHandler for BrowserExportCookiesCommand {
    fn requires_idle_thread(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _incoming: &IncomingMessage,
        thread_context: &mut Thread,
        _thread_runtime: Option<&ThreadRuntime>,
    ) -> Result<CommandReply> {
        if invocation.raw_arguments().trim().is_empty() {
            return Ok(Self::usage(invocation.name()));
        }

        let Some(manager) = self.tools.browser_session_manager().await else {
            return Ok(CommandReply::failed(
                invocation.name(),
                "browser runtime is not registered in the current process",
            ));
        };

        let export_path = Path::new(invocation.raw_arguments().trim());
        let result = manager
            .export_cookies(&thread_context.locator.thread_id, export_path)
            .await?;
        info!(
            thread_id = %thread_context.locator.thread_id,
            external_thread_id = %thread_context.locator.external_thread_id,
            export_path = %result.path,
            cookie_count = result.cookie_count,
            "exported browser cookies by slash command"
        );
        Ok(CommandReply::success(
            invocation.name(),
            format!(
                "exported {} cookies to {}",
                result.cookie_count, result.path
            ),
        ))
    }
}

fn normalize_command_name(name: &str) -> Result<String> {
    let normalized = name.trim().trim_start_matches('/').to_ascii_lowercase();
    if normalized.is_empty() {
        bail!("command name must not be empty");
    }
    if normalized.chars().any(char::is_whitespace) {
        bail!("command name must not contain whitespace");
    }
    Ok(normalized)
}

fn context_budget_estimator() -> ContextBudgetEstimator {
    if let Some(config) = try_global_config() {
        return ContextBudgetEstimator::from_config(
            config.llm_config(),
            config.agent_config().compact_config(),
        );
    }

    let default_config = AppConfig::default();
    ContextBudgetEstimator::from_config(
        default_config.llm_config(),
        default_config.agent_config().compact_config(),
    )
}

fn format_context_summary(
    thread_context: &Thread,
    message_count: usize,
    report: &ContextBudgetReport,
) -> String {
    format!(
        "thread=`{external_thread_id}`\npersisted_messages={message_count}\ntotal_estimated_tokens={total_estimated_tokens}/{context_window_tokens} ({utilization_percent:.2}%)\nsystem_tokens={system_tokens}, chat_tokens={chat_tokens}, visible_tool_tokens={visible_tool_tokens}, reserved_output_tokens={reserved_output_tokens}",
        external_thread_id = thread_context.locator.external_thread_id,
        total_estimated_tokens = report.total_estimated_tokens,
        context_window_tokens = report.context_window_tokens,
        utilization_percent = report.utilization_ratio * 100.0,
        system_tokens = report.system_tokens(),
        chat_tokens = report.chat_tokens(),
        visible_tool_tokens = report.visible_tool_tokens(),
        reserved_output_tokens = report.reserved_output_tokens(),
    )
}

fn format_context_role_report(
    thread_context: &Thread,
    messages: &[ChatMessage],
    estimator: &ContextBudgetEstimator,
    report: &ContextBudgetReport,
) -> String {
    let context_window_tokens = estimator.context_window_tokens().max(1);
    let mut lines = Vec::with_capacity(16);
    lines.push(format!(
        "thread=`{}`",
        thread_context.locator.external_thread_id
    ));
    lines.push(String::new());
    lines.push("message_role".to_string());
    lines.push("| role | tokens | window_ratio |".to_string());
    lines.push("| --- | ---: | ---: |".to_string());

    for role in ordered_chat_message_roles() {
        let role_tokens = estimate_tokens_for_role(messages, estimator, &role);
        let ratio_percent = role_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "| {role} | {role_tokens} | {ratio_percent:.2}% |",
            role = role.as_label(),
        ));
    }

    lines.push(String::new());
    lines.push("context_token_kind".to_string());
    lines.push("| kind | tokens | window_ratio |".to_string());
    lines.push("| --- | ---: | ---: |".to_string());
    for kind in ContextTokenKind::ALL {
        let kind_tokens = report.tokens(kind);
        let ratio_percent = kind_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "| {kind} | {kind_tokens} | {ratio_percent:.2}% |",
            kind = kind.as_str(),
        ));
    }

    lines.join("\n")
}

fn format_context_detail_report(
    thread_context: &Thread,
    messages: &[ChatMessage],
    estimator: &ContextBudgetEstimator,
    requested_count: usize,
) -> String {
    let context_window_tokens = estimator.context_window_tokens().max(1);
    let detail_count = requested_count.min(messages.len());
    let start_index = messages.len().saturating_sub(detail_count);
    let selected_messages = &messages[start_index..];

    let mut lines = Vec::with_capacity(selected_messages.len() + 4);
    lines.push(format!(
        "thread=`{}`",
        thread_context.locator.external_thread_id
    ));
    lines.push(format!(
        "persisted_messages={}\ncontext_window_tokens={}\ndetail_count={}",
        messages.len(),
        context_window_tokens,
        detail_count,
    ));

    if selected_messages.is_empty() {
        lines.push("no persisted messages selected for detail output".to_string());
        return lines.join("\n");
    }

    lines.push(format!(
        "showing_message_range={}..{}",
        start_index + 1,
        start_index + detail_count,
    ));

    for (offset, message) in selected_messages.iter().enumerate() {
        let estimated_tokens = estimator.estimate_message(message);
        let ratio_percent = estimated_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "{index}. role={role} tokens={estimated_tokens} window_ratio={ratio_percent:.2}% preview=\"{preview}\"",
            index = start_index + offset + 1,
            role = message.role.as_label(),
            preview = message_preview(&message.content),
        ));
    }

    lines.join("\n")
}

fn ordered_chat_message_roles() -> [ChatMessageRole; 6] {
    [
        ChatMessageRole::System,
        ChatMessageRole::User,
        ChatMessageRole::Assistant,
        ChatMessageRole::Reasoning,
        ChatMessageRole::Toolcall,
        ChatMessageRole::ToolResult,
    ]
}

fn estimate_tokens_for_role(
    messages: &[ChatMessage],
    estimator: &ContextBudgetEstimator,
    role: &ChatMessageRole,
) -> usize {
    messages
        .iter()
        .filter(|message| &message.role == role)
        .map(|message| estimator.estimate_message(message))
        .sum::<usize>()
}

fn message_preview(content: &str) -> String {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "<empty>".to_string();
    }

    let total_chars = normalized.chars().count();
    let mut preview = normalized
        .chars()
        .take(CONTEXT_MESSAGE_PREVIEW_LIMIT)
        .collect::<String>()
        .replace('"', "'");
    if total_chars > CONTEXT_MESSAGE_PREVIEW_LIMIT {
        preview.push_str("...");
    }
    preview
}

fn remove_prefix_at_if_exist(incoming: &IncomingMessage) -> String {
    // Feishu group chats may prepend one visible mention token before the real slash command.
    // Example: `@_user_1 /echo zxf` should be normalized to `/echo zxf` before command match.
    if incoming.channel != "feishu" {
        return incoming.content.clone();
    }

    let trimmed = incoming.content.trim_start();
    if !trimmed.starts_with('@') {
        return incoming.content.clone();
    }

    let Some(mention_end) = trimmed.find(char::is_whitespace) else {
        return incoming.content.clone();
    };
    let rest = trimmed[mention_end..].trim_start();
    if rest.starts_with('/') {
        rest.to_string()
    } else {
        incoming.content.clone()
    }
}
