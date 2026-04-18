//! Builtin subagent-management tools exposed only to main threads.

use super::{
    ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    empty_tool_input_schema, parse_tool_arguments, tool_definition_from_args,
};
use crate::{
    agent::{SubagentRequest, SubagentRunner},
    session::{SessionManager, ThreadLocator},
    thread::{SubagentSpawnMode, ThreadAgentKind},
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Weak};
use tracing::{info, warn};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnSubagentArguments {
    subagent_key: String,
    content: String,
    #[serde(default = "default_spawn_mode")]
    spawn_mode: SubagentSpawnMode,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendSubagentArguments {
    subagent_key: String,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseSubagentArguments {
    subagent_key: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
struct ListSubagentArguments {}

fn default_spawn_mode() -> SubagentSpawnMode {
    SubagentSpawnMode::Persist
}

#[derive(Clone)]
struct SubagentToolBase {
    runner: Weak<SubagentRunner>,
}

impl SubagentToolBase {
    fn new(runner: Weak<SubagentRunner>) -> Self {
        Self { runner }
    }

    fn upgrade_runner(&self) -> Result<Arc<SubagentRunner>> {
        self.runner
            .upgrade()
            .ok_or_else(|| anyhow!("subagent runner is not available"))
    }

    fn require_parent_context(
        &self,
        context: &ToolCallContext,
        tool_name: &str,
    ) -> Result<(SessionManager, ThreadLocator)> {
        let sessions = context
            .sessions()
            .cloned()
            .with_context(|| format!("subagent tool `{tool_name}` requires session access"))?;
        let locator = context
            .locator()
            .with_context(|| format!("subagent tool `{tool_name}` requires thread locator"))?;
        let locator = ThreadLocator::try_from(locator)?;
        if locator.child_thread.is_some() {
            bail!("subagent tool `{tool_name}` is only available on parent threads");
        }
        Ok((sessions, locator))
    }

    fn resolve_subagent_kind(&self, subagent_key: &str) -> Result<ThreadAgentKind> {
        ThreadAgentKind::from_subagent_key(subagent_key).ok_or_else(|| {
            anyhow!(
                "unsupported subagent profile `{}`; expected one of the registered subagent keys",
                subagent_key
            )
        })
    }

    fn child_locator(
        &self,
        parent_locator: &ThreadLocator,
        subagent_key: &str,
        spawn_mode: SubagentSpawnMode,
    ) -> ThreadLocator {
        ThreadLocator::for_child(parent_locator, subagent_key, spawn_mode)
    }

    fn require_non_empty_content(&self, content: &str, tool_name: &str) -> Result<()> {
        if content.trim().is_empty() {
            bail!("{tool_name} requires non-empty content");
        }
        Ok(())
    }

    async fn existing_child_mode(
        &self,
        sessions: &SessionManager,
        child_locator: &ThreadLocator,
    ) -> Result<Option<SubagentSpawnMode>> {
        let child_thread = sessions.load_thread(child_locator).await?;
        Ok(child_thread
            .and_then(|thread| thread.child_thread_identity().map(|child| child.spawn_mode)))
    }

    async fn require_existing_persist_child(
        &self,
        sessions: &SessionManager,
        parent_locator: &ThreadLocator,
        subagent_key: &str,
        tool_name: &str,
        require_initialized: bool,
    ) -> Result<ThreadLocator> {
        let child_locator =
            self.child_locator(parent_locator, subagent_key, SubagentSpawnMode::Persist);
        let Some(child_thread) = sessions.load_thread(&child_locator).await? else {
            bail!(
                "{tool_name} requires an existing persist subagent `{subagent_key}`; call spawn_subagent first"
            );
        };
        let Some(child_identity) = child_thread.child_thread_identity() else {
            bail!(
                "{tool_name} found child thread `{}` without persisted child identity",
                child_locator.thread_id
            );
        };
        if child_identity.spawn_mode != SubagentSpawnMode::Persist {
            bail!(
                "{tool_name} only supports persist subagents; `{subagent_key}` is currently `{}`",
                child_identity.spawn_mode.as_str()
            );
        }
        if require_initialized && !child_thread.is_initialized() {
            bail!(
                "persist subagent `{subagent_key}` is not available; call spawn_subagent to reinitialize it first"
            );
        }
        Ok(child_locator)
    }
}

async fn cleanup_yolo_child_thread(
    sessions: &SessionManager,
    child_locator: &ThreadLocator,
    subagent_key: &str,
) -> (bool, Option<String>) {
    match sessions.remove_thread(child_locator).await {
        Ok(removed) => (removed, None),
        Err(error) => {
            warn!(
                thread_id = %child_locator.thread_id,
                subagent_key = %subagent_key,
                error = %error,
                "best-effort yolo child-thread cleanup failed"
            );
            (false, Some(error.to_string()))
        }
    }
}

pub(crate) async fn register_subagent_tools(
    registry: &super::ToolRegistry,
    runner: Weak<SubagentRunner>,
) {
    registry
        .register_if_missing(Arc::new(SpawnSubagentTool::new(runner.clone())))
        .await;
    registry
        .register_if_missing(Arc::new(SendSubagentTool::new(runner.clone())))
        .await;
    registry
        .register_if_missing(Arc::new(CloseSubagentTool::new(runner.clone())))
        .await;
    registry
        .register_if_missing(Arc::new(ListSubagentTool::new(runner)))
        .await;
}

struct SpawnSubagentTool {
    base: SubagentToolBase,
}

impl SpawnSubagentTool {
    fn new(runner: Weak<SubagentRunner>) -> Self {
        Self {
            base: SubagentToolBase::new(runner),
        }
    }
}

struct SendSubagentTool {
    base: SubagentToolBase,
}

impl SendSubagentTool {
    fn new(runner: Weak<SubagentRunner>) -> Self {
        Self {
            base: SubagentToolBase::new(runner),
        }
    }
}

struct CloseSubagentTool {
    base: SubagentToolBase,
}

impl CloseSubagentTool {
    fn new(runner: Weak<SubagentRunner>) -> Self {
        Self {
            base: SubagentToolBase::new(runner),
        }
    }
}

struct ListSubagentTool {
    base: SubagentToolBase,
}

impl ListSubagentTool {
    fn new(runner: Weak<SubagentRunner>) -> Self {
        Self {
            base: SubagentToolBase::new(runner),
        }
    }
}

#[async_trait]
impl ToolHandler for SpawnSubagentTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<SpawnSubagentArguments>(
            "spawn_subagent",
            "Create or reuse one named subagent child thread, immediately execute the first task, and return the aggregated result.",
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
        let args: SpawnSubagentArguments = parse_tool_arguments(request, "spawn_subagent")?;
        self.base
            .require_non_empty_content(&args.content, "spawn_subagent")?;

        let runner = self.base.upgrade_runner()?;
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "spawn_subagent")?;
        let thread_agent_kind = self.base.resolve_subagent_kind(&args.subagent_key)?;
        let child_locator =
            self.base
                .child_locator(&parent_locator, &args.subagent_key, args.spawn_mode);

        if let Some(existing_mode) = self
            .base
            .existing_child_mode(&sessions, &child_locator)
            .await?
            && existing_mode != args.spawn_mode
        {
            bail!(
                "subagent `{}` already exists in `{}` mode; mixed-mode reuse is not supported",
                args.subagent_key,
                existing_mode.as_str()
            );
        }

        info!(
            parent_thread_id = %parent_locator.thread_id,
            child_thread_id = %child_locator.thread_id,
            subagent_key = %args.subagent_key,
            spawn_mode = %args.spawn_mode.as_str(),
            "spawning subagent and executing first task"
        );
        let child_locator = sessions
            .create_thread_at(&child_locator, Utc::now(), thread_agent_kind)
            .await?;
        let run_result = runner
            .run(SubagentRequest {
                parent_locator: parent_locator.clone(),
                child_locator: child_locator.clone(),
                prompt: args.content,
                sessions: sessions.clone(),
            })
            .await;

        let (cleanup_removed, cleanup_error) = if args.spawn_mode == SubagentSpawnMode::Yolo {
            cleanup_yolo_child_thread(&sessions, &child_locator, &args.subagent_key).await
        } else {
            (false, None)
        };

        let result = run_result?;
        Ok(ToolCallResult {
            content: result.output.reply.clone(),
            metadata: json!({
                "event_kind": "spawn_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": args.spawn_mode.as_str(),
                "thread_id": child_locator.thread_id.to_string(),
                "request_status": if result.output.succeeded { "succeeded" } else { "failed" },
                "internal_dispatch_event_count": result.dispatch_events.len(),
                "available": args.spawn_mode == SubagentSpawnMode::Persist,
                "cleanup_removed": cleanup_removed,
                "cleanup_error": cleanup_error,
            }),
            is_error: !result.output.succeeded,
        })
    }
}

#[async_trait]
impl ToolHandler for SendSubagentTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<SendSubagentArguments>(
            "send_subagent",
            "Send one follow-up task to an existing persist subagent child thread and return the aggregated result.",
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
        let args: SendSubagentArguments = parse_tool_arguments(request, "send_subagent")?;
        self.base
            .require_non_empty_content(&args.content, "send_subagent")?;

        let runner = self.base.upgrade_runner()?;
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "send_subagent")?;
        let _ = self.base.resolve_subagent_kind(&args.subagent_key)?;
        let child_locator = self
            .base
            .require_existing_persist_child(
                &sessions,
                &parent_locator,
                &args.subagent_key,
                "send_subagent",
                true,
            )
            .await?;

        info!(
            parent_thread_id = %parent_locator.thread_id,
            child_thread_id = %child_locator.thread_id,
            subagent_key = %args.subagent_key,
            "sending follow-up task to persist subagent"
        );
        let result = runner
            .run(SubagentRequest {
                parent_locator: parent_locator.clone(),
                child_locator: child_locator.clone(),
                prompt: args.content,
                sessions: sessions.clone(),
            })
            .await?;

        Ok(ToolCallResult {
            content: result.output.reply.clone(),
            metadata: json!({
                "event_kind": "send_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": SubagentSpawnMode::Persist.as_str(),
                "thread_id": child_locator.thread_id.to_string(),
                "request_status": if result.output.succeeded { "succeeded" } else { "failed" },
                "internal_dispatch_event_count": result.dispatch_events.len(),
            }),
            is_error: !result.output.succeeded,
        })
    }
}

#[async_trait]
impl ToolHandler for CloseSubagentTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<CloseSubagentArguments>(
            "close_subagent",
            "Close one existing persist subagent child thread while keeping its stable child-thread identity for future respawn flows.",
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
        let args: CloseSubagentArguments = parse_tool_arguments(request, "close_subagent")?;
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "close_subagent")?;
        let child_locator = self.base.child_locator(
            &parent_locator,
            &args.subagent_key,
            SubagentSpawnMode::Persist,
        );

        let Some(existing_mode) = self
            .base
            .existing_child_mode(&sessions, &child_locator)
            .await?
        else {
            return Ok(ToolCallResult {
                content: format!("Subagent `{}` was not found.", args.subagent_key),
                metadata: json!({
                    "event_kind": "close_subagent",
                    "subagent_key": args.subagent_key,
                    "closed": false,
                }),
                is_error: false,
            });
        };
        if existing_mode != SubagentSpawnMode::Persist {
            bail!(
                "close_subagent only supports persist subagents; `{}` is currently `{}`",
                args.subagent_key,
                existing_mode.as_str()
            );
        }

        info!(
            parent_thread_id = %parent_locator.thread_id,
            child_thread_id = %child_locator.thread_id,
            subagent_key = %args.subagent_key,
            "closing persist subagent"
        );
        let Some(mut child_thread) = sessions.lock_thread(&child_locator, Utc::now()).await? else {
            return Ok(ToolCallResult {
                content: format!("Subagent `{}` was not found.", args.subagent_key),
                metadata: json!({
                    "event_kind": "close_subagent",
                    "subagent_key": args.subagent_key,
                    "closed": false,
                }),
                is_error: false,
            });
        };
        if !child_thread.is_initialized() {
            return Ok(ToolCallResult {
                content: format!(
                    "Persist subagent `{}` was already closed.",
                    args.subagent_key
                ),
                metadata: json!({
                    "event_kind": "close_subagent",
                    "subagent_key": args.subagent_key,
                    "spawn_mode": SubagentSpawnMode::Persist.as_str(),
                    "closed": false,
                    "already_closed": true,
                }),
                is_error: false,
            });
        }
        child_thread
            .reset_to_initial_state_preserving_child_thread(Utc::now())
            .await?;
        Ok(ToolCallResult {
            content: format!(
                "Persist subagent `{}` was closed and must be spawned again before reuse.",
                args.subagent_key
            ),
            metadata: json!({
                "event_kind": "close_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": SubagentSpawnMode::Persist.as_str(),
                "closed": true,
            }),
            is_error: false,
        })
    }
}

#[async_trait]
impl ToolHandler for ListSubagentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_subagent".to_string(),
            description:
                "List subagent child threads that currently belong to the active parent thread."
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
        let _: ListSubagentArguments = parse_tool_arguments(request, "list_subagent")?;
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "list_subagent")?;
        let records = sessions.list_child_threads(&parent_locator).await?;
        let subagents = records
            .into_iter()
            .filter_map(|record| {
                let child = record.snapshot.state.child_thread?;
                let available = child.spawn_mode == SubagentSpawnMode::Persist
                    && record.snapshot.state.lifecycle.initialized;
                Some(json!({
                    "subagent_key": child.subagent_key,
                    "spawn_mode": child.spawn_mode.as_str(),
                    "thread_id": record.locator.thread_id,
                    "available": available,
                }))
            })
            .collect::<Vec<_>>();
        let content = if subagents.is_empty() {
            "No subagents are currently prepared for this parent thread.".to_string()
        } else {
            subagents
                .iter()
                .map(|value| {
                    format!(
                        "- {} [{}] available={}",
                        value["subagent_key"].as_str().unwrap_or("unknown"),
                        value["spawn_mode"].as_str().unwrap_or("unknown"),
                        value["available"].as_bool().unwrap_or(false)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolCallResult {
            content,
            metadata: json!({
                "event_kind": "list_subagent",
                "subagents": subagents,
            }),
            is_error: false,
        })
    }
}
