//! Builtin tool handlers exposed for command-session execution.

use super::{
    CommandSessionManager, CommandWriteRequest, format_task_listing,
    session::CommandExecutionRequest,
};
use crate::agent::tool::{
    ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    empty_tool_input_schema, parse_tool_arguments, tool_definition_from_args,
};
use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecCommandArguments {
    cmd: String,
    workdir: Option<PathBuf>,
    shell: Option<String>,
    #[serde(default)]
    tty: bool,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteStdinArguments {
    session_id: String,
    #[serde(default)]
    chars: String,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
struct ListUnreadCommandTasksArguments {}

/// Builtin tool that starts one command and optionally keeps it alive in the background.
pub struct ExecCommandTool {
    sessions: Arc<CommandSessionManager>,
}

impl ExecCommandTool {
    /// Create one `exec_command` tool backed by the shared session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::ExecCommandTool;
    ///
    /// let _tool = ExecCommandTool::new();
    /// ```
    pub fn new() -> Self {
        Self::with_sessions(Arc::new(CommandSessionManager::new()))
    }

    pub fn with_sessions(sessions: Arc<CommandSessionManager>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl ToolHandler for ExecCommandTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ExecCommandArguments>(
            "exec_command",
            "Run one shell command with optional workdir, shell, tty, and background session continuation.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        self.call_with_context(ToolCallContext::default(), request)
            .await
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let args: ExecCommandArguments = parse_tool_arguments(request, "exec_command")?;
        let result = self
            .sessions
            .exec_command_from_context(
                context.thread_id(),
                CommandExecutionRequest {
                    cmd: args.cmd,
                    workdir: args.workdir,
                    shell: args.shell,
                    tty: args.tty,
                    yield_time_ms: args.yield_time_ms.unwrap_or(1_000),
                    max_output_tokens: args.max_output_tokens,
                },
            )
            .await?;
        Ok(result.into_tool_result("exec_command"))
    }
}

/// Builtin tool that writes stdin to one existing command session or polls output with empty input.
pub struct WriteStdinTool {
    sessions: Arc<CommandSessionManager>,
}

impl WriteStdinTool {
    /// Create one `write_stdin` tool backed by the shared session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::WriteStdinTool;
    ///
    /// let _tool = WriteStdinTool::new();
    /// ```
    pub fn new() -> Self {
        Self::with_sessions(Arc::new(CommandSessionManager::new()))
    }

    pub fn with_sessions(sessions: Arc<CommandSessionManager>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl ToolHandler for WriteStdinTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<WriteStdinArguments>(
            "write_stdin",
            "Write stdin into one running command session, or poll with empty chars to fetch the next output chunk.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        self.call_with_context(ToolCallContext::default(), request)
            .await
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let args: WriteStdinArguments = parse_tool_arguments(request, "write_stdin")?;
        let result = self
            .sessions
            .write_stdin_from_context(
                context.thread_id(),
                CommandWriteRequest {
                    session_id: args.session_id,
                    chars: args.chars,
                    yield_time_ms: args.yield_time_ms.unwrap_or(1_000),
                    max_output_tokens: args.max_output_tokens,
                },
            )
            .await?;
        Ok(result.into_tool_result("write_stdin"))
    }
}

/// Builtin tool that lists unread command-session output for the current thread.
pub struct ListUnreadCommandTasksTool {
    sessions: Arc<CommandSessionManager>,
}

impl ListUnreadCommandTasksTool {
    /// Create one `list_unread_command_tasks` tool backed by the shared session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::ListUnreadCommandTasksTool;
    ///
    /// let _tool = ListUnreadCommandTasksTool::new();
    /// ```
    pub fn new() -> Self {
        Self::with_sessions(Arc::new(CommandSessionManager::new()))
    }

    pub fn with_sessions(sessions: Arc<CommandSessionManager>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl ToolHandler for ListUnreadCommandTasksTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_unread_command_tasks".to_string(),
            description:
                "List command sessions in the current thread that still have unread output chunks."
                    .to_string(),
            input_schema: empty_tool_input_schema(),
            source: crate::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        self.call_with_context(ToolCallContext::default(), request)
            .await
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let started_at = Instant::now();
        let _: ListUnreadCommandTasksArguments =
            parse_tool_arguments(request, "list_unread_command_tasks")?;
        let tasks = self
            .sessions
            .list_unread_tasks_from_context(context.thread_id())
            .await;
        Ok(ToolCallResult {
            content: format_task_listing(&tasks),
            metadata: json!({
                "event_kind": "list_unread_command_tasks",
                "wall_time_seconds": started_at.elapsed().as_secs_f64(),
                "tasks": tasks,
            }),
            is_error: false,
        })
    }
}

impl Default for ExecCommandTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for WriteStdinTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ListUnreadCommandTasksTool {
    fn default() -> Self {
        Self::new()
    }
}
