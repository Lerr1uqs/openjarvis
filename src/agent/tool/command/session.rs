//! Thread-scoped session manager for background and interactive command execution.

use super::{
    output::{format_command_summary, truncate_output},
    process::{CommandLaunchOptions, ProcessEvent, spawn_command},
};
use crate::agent::ToolCallResult;
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::{
    sync::{Mutex, RwLock, mpsc},
    time::{Duration, Instant as TokioInstant, timeout},
};
use tracing::debug;
use uuid::Uuid;

const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
const DEFAULT_COMMAND_THREAD_ID: &str = "__standalone_command_thread__";

/// One command execution request accepted by the command session runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandExecutionRequest {
    pub cmd: String,
    pub workdir: Option<std::path::PathBuf>,
    pub shell: Option<String>,
    pub tty: bool,
    pub yield_time_ms: u64,
    pub max_output_tokens: Option<usize>,
}

impl CommandExecutionRequest {
    /// Build one request with sensible defaults for the shared runtime.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandExecutionRequest;
    ///
    /// let request = CommandExecutionRequest::new("printf 'hello'");
    /// assert_eq!(request.yield_time_ms, 1_000);
    /// assert!(!request.tty);
    /// ```
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            workdir: None,
            shell: None,
            tty: false,
            yield_time_ms: DEFAULT_YIELD_TIME_MS,
            max_output_tokens: None,
        }
    }
}

/// One follow-up stdin write request for an existing command session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandWriteRequest {
    pub session_id: String,
    pub chars: String,
    pub yield_time_ms: u64,
    pub max_output_tokens: Option<usize>,
}

impl CommandWriteRequest {
    /// Build one follow-up request for an existing session id.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandWriteRequest;
    ///
    /// let request = CommandWriteRequest::new("session-1");
    /// assert_eq!(request.session_id, "session-1");
    /// ```
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            chars: String::new(),
            yield_time_ms: DEFAULT_YIELD_TIME_MS,
            max_output_tokens: None,
        }
    }
}

/// Exported command task state for runtime inspection and list queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandTaskSnapshot {
    pub thread_id: String,
    pub session_id: String,
    pub status: CommandTaskStatus,
    pub command: String,
    pub has_unread_output: bool,
    pub exit_code: Option<i32>,
    pub updated_at: DateTime<Utc>,
}

/// Stable task status exposed by the command session runtime.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandTaskStatus {
    Doing,
    Done,
}

impl CommandTaskStatus {
    /// Return the stable string form used in tool summaries and snapshots.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandTaskStatus;
    ///
    /// assert_eq!(CommandTaskStatus::Doing.as_str(), "Doing");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Doing => "Doing",
            Self::Done => "Done",
        }
    }
}

/// One normalized execution summary returned by `exec_command` and `write_stdin`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandExecutionResult {
    pub command: String,
    pub chunk_id: u64,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub session_id: Option<String>,
    pub original_token_count: usize,
    pub output: String,
    pub running: bool,
}

impl CommandExecutionResult {
    pub(crate) fn into_tool_result(self, event_kind: &'static str) -> ToolCallResult {
        ToolCallResult {
            content: format_command_summary(&self),
            metadata: json!({
                "event_kind": event_kind,
                "command": self.command,
                "chunk_id": self.chunk_id,
                "wall_time_seconds": self.wall_time_seconds,
                "exit_code": self.exit_code,
                "session_id": self.session_id,
                "original_token_count": self.original_token_count,
                "running": self.running,
                "output": self.output,
            }),
            is_error: false,
        }
    }
}

struct CommandSessionRecord {
    snapshot: RwLock<CommandTaskSnapshot>,
    runtime: Mutex<CommandSessionRuntime>,
}

struct CommandSessionRuntime {
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
    event_rx: mpsc::UnboundedReceiver<ProcessEvent>,
    next_chunk_id: u64,
    pending_output: String,
}

/// Shared thread-scoped command session owner used by builtin command tools.
pub struct CommandSessionManager {
    sessions: RwLock<HashMap<String, Arc<CommandSessionRecord>>>,
}

impl CommandSessionManager {
    /// Create an empty command session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandSessionManager;
    ///
    /// let manager = CommandSessionManager::new();
    /// assert!(manager.export_task_snapshots_blocking().is_empty());
    /// ```
    pub fn new() -> Self {
        debug!("initialized command session manager");
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Execute one command inside the provided thread scope.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::tool::command::{CommandExecutionRequest, CommandSessionManager};
    ///
    /// let manager = CommandSessionManager::new();
    /// let result = manager
    ///     .exec_command("thread-demo", CommandExecutionRequest::new("printf 'hello'"))
    ///     .await?;
    /// assert!(result.output.contains("hello"));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn exec_command(
        &self,
        thread_id: impl AsRef<str>,
        request: CommandExecutionRequest,
    ) -> Result<CommandExecutionResult> {
        self.exec_command_with_options(thread_id, request, CommandLaunchOptions::default())
            .await
    }

    pub(crate) async fn exec_command_with_options(
        &self,
        thread_id: impl AsRef<str>,
        request: CommandExecutionRequest,
        launch_options: CommandLaunchOptions,
    ) -> Result<CommandExecutionResult> {
        let call_started_at = Instant::now();
        if request.cmd.trim().is_empty() {
            bail!("exec_command requires a non-empty `cmd`");
        }

        let thread_id = normalize_thread_id(Some(thread_id.as_ref()));
        debug!(
            thread_id = %thread_id,
            command = %request.cmd,
            tty = request.tty,
            workdir = ?request.workdir,
            shell = ?request.shell,
            "starting command session"
        );
        let spawned = spawn_command(&request, &launch_options).await?;
        let session_id = format!("command-session-{}", Uuid::new_v4());
        let snapshot = CommandTaskSnapshot {
            thread_id: thread_id.clone(),
            session_id: session_id.clone(),
            status: CommandTaskStatus::Doing,
            command: request.cmd.clone(),
            has_unread_output: false,
            exit_code: None,
            updated_at: Utc::now(),
        };
        let record = Arc::new(CommandSessionRecord {
            snapshot: RwLock::new(snapshot),
            runtime: Mutex::new(CommandSessionRuntime {
                input_tx: spawned.input_tx,
                event_rx: spawned.event_rx,
                next_chunk_id: 0,
                pending_output: String::new(),
            }),
        });
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&record));
        self.collect_result(
            record,
            request.yield_time_ms,
            request.max_output_tokens,
            call_started_at,
        )
        .await
    }

    /// Continue one existing command session by optionally writing stdin and polling output.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::tool::command::{
    ///     CommandExecutionRequest, CommandSessionManager, CommandWriteRequest,
    /// };
    ///
    /// let manager = CommandSessionManager::new();
    /// let started = manager
    ///     .exec_command("thread-demo", CommandExecutionRequest::new("cat"))
    ///     .await?;
    /// if let Some(session_id) = started.session_id {
    ///     let mut write = CommandWriteRequest::new(session_id);
    ///     write.chars = "hello\n".to_string();
    ///     let _ = manager.write_stdin("thread-demo", write).await?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn write_stdin(
        &self,
        thread_id: impl AsRef<str>,
        request: CommandWriteRequest,
    ) -> Result<CommandExecutionResult> {
        let call_started_at = Instant::now();
        let thread_id = normalize_thread_id(Some(thread_id.as_ref()));
        let record = self
            .sessions
            .read()
            .await
            .get(&request.session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown command session `{}`", request.session_id))?;

        let snapshot = record.snapshot.read().await.clone();
        if snapshot.thread_id != thread_id {
            bail!(
                "command session `{}` does not belong to thread `{}`",
                request.session_id,
                thread_id
            );
        }

        if snapshot.status == CommandTaskStatus::Done && !request.chars.is_empty() {
            bail!(
                "command session `{}` has already exited with code {}",
                request.session_id,
                snapshot.exit_code.unwrap_or_default()
            );
        }

        if !request.chars.is_empty() {
            debug!(
                thread_id = %thread_id,
                session_id = %request.session_id,
                byte_count = request.chars.len(),
                "writing command session stdin"
            );
            let runtime = record.runtime.lock().await;
            runtime
                .input_tx
                .send(request.chars.into_bytes())
                .map_err(|_| {
                    anyhow::anyhow!("failed to write stdin because the command session is closed")
                })?;
        }

        self.collect_result(
            record,
            request.yield_time_ms,
            request.max_output_tokens,
            call_started_at,
        )
        .await
    }

    /// Return the current unread-output tasks for one internal thread.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandSessionManager;
    ///
    /// let manager = CommandSessionManager::new();
    /// assert!(manager.list_unread_tasks_blocking("thread-demo").is_empty());
    /// ```
    pub async fn list_unread_tasks(&self, thread_id: impl AsRef<str>) -> Vec<CommandTaskSnapshot> {
        let thread_id = normalize_thread_id(Some(thread_id.as_ref()));
        debug!(thread_id = %thread_id, "listing unread command tasks");
        let sessions = self.session_records().await;
        let mut tasks = Vec::new();
        for record in sessions {
            self.refresh_record_snapshot(&record).await;
            let snapshot = record.snapshot.read().await.clone();
            if snapshot.thread_id == thread_id && snapshot.has_unread_output {
                tasks.push(snapshot);
            }
        }
        tasks.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        tasks
    }

    /// Export all known task snapshots, including both running and completed sessions.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::command::CommandSessionManager;
    ///
    /// let manager = CommandSessionManager::new();
    /// assert!(manager.export_task_snapshots_blocking().is_empty());
    /// ```
    pub async fn export_task_snapshots(&self) -> Vec<CommandTaskSnapshot> {
        debug!("exporting command task snapshots");
        let sessions = self.session_records().await;
        let mut tasks = Vec::new();
        for record in sessions {
            self.refresh_record_snapshot(&record).await;
            tasks.push(record.snapshot.read().await.clone());
        }
        tasks.sort_by(|left, right| {
            left.thread_id
                .cmp(&right.thread_id)
                .then(left.session_id.cmp(&right.session_id))
        });
        tasks
    }

    /// Blocking helper for simple assertions outside async contexts.
    pub fn list_unread_tasks_blocking(
        &self,
        thread_id: impl AsRef<str>,
    ) -> Vec<CommandTaskSnapshot> {
        let thread_id = normalize_thread_id(Some(thread_id.as_ref()));
        let sessions = self.sessions.blocking_read();
        let mut tasks = Vec::new();
        for record in sessions.values() {
            let snapshot = record.snapshot.blocking_read().clone();
            if snapshot.thread_id == thread_id && snapshot.has_unread_output {
                tasks.push(snapshot);
            }
        }
        tasks.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        tasks
    }

    /// Blocking helper for simple assertions outside async contexts.
    pub fn export_task_snapshots_blocking(&self) -> Vec<CommandTaskSnapshot> {
        self.sessions
            .blocking_read()
            .values()
            .map(|record| record.snapshot.blocking_read().clone())
            .collect()
    }

    pub(crate) async fn exec_command_from_context(
        &self,
        thread_id: Option<&str>,
        request: CommandExecutionRequest,
    ) -> Result<CommandExecutionResult> {
        self.exec_command_from_context_with_options(
            thread_id,
            request,
            CommandLaunchOptions::default(),
        )
        .await
    }

    pub(crate) async fn exec_command_from_context_with_options(
        &self,
        thread_id: Option<&str>,
        request: CommandExecutionRequest,
        launch_options: CommandLaunchOptions,
    ) -> Result<CommandExecutionResult> {
        self.exec_command_with_options(normalize_thread_id(thread_id), request, launch_options)
            .await
    }

    pub(crate) async fn write_stdin_from_context(
        &self,
        thread_id: Option<&str>,
        request: CommandWriteRequest,
    ) -> Result<CommandExecutionResult> {
        self.write_stdin(normalize_thread_id(thread_id), request)
            .await
    }

    pub(crate) async fn list_unread_tasks_from_context(
        &self,
        thread_id: Option<&str>,
    ) -> Vec<CommandTaskSnapshot> {
        self.list_unread_tasks(normalize_thread_id(thread_id)).await
    }

    async fn collect_result(
        &self,
        record: Arc<CommandSessionRecord>,
        yield_time_ms: u64,
        max_output_tokens: Option<usize>,
        call_started_at: Instant,
    ) -> Result<CommandExecutionResult> {
        let deadline = TokioInstant::now() + Duration::from_millis(yield_time_ms);
        let mut saw_exit = false;
        let mut runtime = record.runtime.lock().await;
        self.drain_available_events(&record, &mut runtime).await;

        while TokioInstant::now() < deadline {
            let remaining = deadline.saturating_duration_since(TokioInstant::now());
            if remaining.is_zero() {
                break;
            }

            match timeout(remaining, runtime.event_rx.recv()).await {
                Ok(Some(event)) => {
                    if matches!(event, ProcessEvent::Exit(_)) {
                        saw_exit = true;
                    }
                    self.apply_event(&record, &mut runtime, event).await;
                    self.drain_available_events(&record, &mut runtime).await;
                    if saw_exit {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        let output = std::mem::take(&mut runtime.pending_output);
        let chunk_id = runtime.next_chunk_id;
        runtime.next_chunk_id += 1;
        let has_pending_output = !runtime.pending_output.is_empty();
        drop(runtime);

        {
            let mut snapshot = record.snapshot.write().await;
            snapshot.has_unread_output = has_pending_output;
        }
        let snapshot = record.snapshot.read().await.clone();
        let (output, original_token_count) = truncate_output(&output, max_output_tokens);
        Ok(CommandExecutionResult {
            command: snapshot.command.clone(),
            chunk_id,
            wall_time_seconds: call_started_at.elapsed().as_secs_f64(),
            exit_code: snapshot.exit_code,
            session_id: (snapshot.status == CommandTaskStatus::Doing)
                .then_some(snapshot.session_id.clone()),
            original_token_count,
            output,
            running: snapshot.status == CommandTaskStatus::Doing,
        })
    }

    async fn drain_available_events(
        &self,
        record: &Arc<CommandSessionRecord>,
        runtime: &mut CommandSessionRuntime,
    ) {
        while let Ok(event) = runtime.event_rx.try_recv() {
            self.apply_event(record, runtime, event).await;
        }
    }

    async fn apply_event(
        &self,
        record: &Arc<CommandSessionRecord>,
        runtime: &mut CommandSessionRuntime,
        event: ProcessEvent,
    ) {
        let mut snapshot = record.snapshot.write().await;
        snapshot.updated_at = Utc::now();
        match event {
            ProcessEvent::Output(chunk) => {
                runtime.pending_output.push_str(&chunk);
                snapshot.has_unread_output = !runtime.pending_output.is_empty();
            }
            ProcessEvent::Exit(exit_code) => {
                snapshot.status = CommandTaskStatus::Done;
                snapshot.exit_code = Some(exit_code);
            }
        }
    }

    async fn refresh_record_snapshot(&self, record: &Arc<CommandSessionRecord>) {
        let mut runtime = record.runtime.lock().await;
        self.drain_available_events(record, &mut runtime).await;
        let has_unread_output = !runtime.pending_output.is_empty();
        drop(runtime);

        let mut snapshot = record.snapshot.write().await;
        snapshot.has_unread_output = has_unread_output;
    }

    async fn session_records(&self) -> Vec<Arc<CommandSessionRecord>> {
        self.sessions.read().await.values().cloned().collect()
    }
}

impl Default for CommandSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_thread_id(thread_id: Option<&str>) -> String {
    thread_id
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .unwrap_or(DEFAULT_COMMAND_THREAD_ID)
        .to_string()
}
