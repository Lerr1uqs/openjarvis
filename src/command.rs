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

use crate::model::IncomingMessage;
use crate::thread::Thread;
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

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
    /// Execute one parsed command invocation and return a formatted reply payload.
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
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
        registry
            .register("test", Arc::new(TestCommand))
            .expect("built-in test command should register");
        registry
            .register("equal", Arc::new(EqualCommand))
            .expect("built-in equal command should register");
        registry
            .register("echo", Arc::new(EchoCommand))
            .expect("built-in echo command should register");
        registry
            .register("clear", Arc::new(ClearCommand))
            .expect("built-in clear command should register");
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

    /// Try to execute one incoming message as a slash command with the resolved target thread context.
    pub async fn try_execute_with_thread_context(
        &self,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
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
                .execute(&invocation, incoming, thread_context)
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
    ) -> Result<CommandReply> {
        Ok(CommandReply::success(
            invocation.name(),
            invocation.raw_arguments().to_string(),
        ))
    }
}

struct ClearCommand;

impl ClearCommand {
    fn usage(name: &str) -> CommandReply {
        CommandReply::failed(name, "usage: /clear")
    }
}

#[async_trait]
impl CommandHandler for ClearCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        incoming: &IncomingMessage,
        thread_context: &mut Thread,
    ) -> Result<CommandReply> {
        if !invocation.arguments().is_empty() {
            return Ok(Self::usage(invocation.name()));
        }

        info!(
            thread_id = %thread_context.locator.thread_id,
            external_thread_id = %thread_context.locator.external_thread_id,
            "clearing thread history and runtime state by command"
        );
        thread_context.clear_to_initial_state(incoming.received_at);
        Ok(CommandReply::success(
            invocation.name(),
            format!(
                "cleared current thread `{}`; all chat messages and thread-scoped runtime state have been reset",
                thread_context.locator.external_thread_id
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
