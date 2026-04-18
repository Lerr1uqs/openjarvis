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
use tracing::warn;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnSubagentArguments {
    subagent_key: String,
    #[serde(default = "default_spawn_mode")]
    spawn_mode: SubagentSpawnMode,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendSubagentArguments {
    subagent_key: String,
    content: String,
    #[serde(default = "default_spawn_mode")]
    spawn_mode: SubagentSpawnMode,
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
            "Prepare one named subagent child thread for the current parent thread, reusing the existing child thread when the same profile was already prepared.",
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
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "spawn_subagent")?;
        let thread_agent_kind = self.base.resolve_subagent_kind(&args.subagent_key)?;
        let child_locator =
            self.base
                .child_locator(&parent_locator, &args.subagent_key, args.spawn_mode);
        let child_locator = sessions
            .create_thread_at(&child_locator, Utc::now(), thread_agent_kind)
            .await?;
        Ok(ToolCallResult {
            content: format!(
                "Subagent `{}` is ready on child thread `{}`.",
                args.subagent_key, child_locator.thread_id
            ),
            metadata: json!({
                "event_kind": "spawn_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": args.spawn_mode.as_str(),
                "thread_id": child_locator.thread_id.to_string(),
                "available": true,
            }),
            is_error: false,
        })
    }
}

#[async_trait]
impl ToolHandler for SendSubagentTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<SendSubagentArguments>(
            "send_subagent",
            "Synchronously execute one subagent request on a dedicated child thread and return the aggregated result in a single tool response.",
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
        if args.content.trim().is_empty() {
            bail!("send_subagent requires non-empty content");
        }

        let runner = self.base.upgrade_runner()?;
        let (sessions, parent_locator) = self
            .base
            .require_parent_context(&context, "send_subagent")?;
        let thread_agent_kind = self.base.resolve_subagent_kind(&args.subagent_key)?;
        let child_locator =
            self.base
                .child_locator(&parent_locator, &args.subagent_key, args.spawn_mode);
        let child_locator = sessions
            .create_thread_at(&child_locator, Utc::now(), thread_agent_kind)
            .await?;
        let result = runner
            .run(SubagentRequest {
                parent_locator: parent_locator.clone(),
                child_locator: child_locator.clone(),
                prompt: args.content,
                sessions: sessions.clone(),
            })
            .await?;

        let mut cleanup_error = None;
        let cleanup_removed =
            if args.spawn_mode == SubagentSpawnMode::Yolo && result.output.succeeded {
                match sessions.remove_thread(&child_locator).await {
                    Ok(removed) => removed,
                    Err(error) => {
                        warn!(
                            thread_id = %child_locator.thread_id,
                            subagent_key = %args.subagent_key,
                            error = %error,
                            "best-effort yolo child-thread cleanup failed"
                        );
                        cleanup_error = Some(error.to_string());
                        false
                    }
                }
            } else {
                false
            };

        Ok(ToolCallResult {
            content: result.output.reply.clone(),
            metadata: json!({
                "event_kind": "send_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": args.spawn_mode.as_str(),
                "thread_id": child_locator.thread_id.to_string(),
                "request_status": if result.output.succeeded { "succeeded" } else { "failed" },
                "internal_dispatch_event_count": result.dispatch_events.len(),
                "cleanup_removed": cleanup_removed,
                "cleanup_error": cleanup_error,
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
            "Stop reusing one prepared subagent child thread while keeping its stable child-thread identity for future re-prepare flows.",
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
        let spawn_mode = child_thread
            .child_thread_identity()
            .map(|child| child.spawn_mode.as_str())
            .unwrap_or(SubagentSpawnMode::Persist.as_str());
        child_thread
            .reset_to_initial_state_preserving_child_thread(Utc::now())
            .await?;
        Ok(ToolCallResult {
            content: format!(
                "Subagent `{}` was closed and must be prepared again before reuse.",
                args.subagent_key
            ),
            metadata: json!({
                "event_kind": "close_subagent",
                "subagent_key": args.subagent_key,
                "spawn_mode": spawn_mode,
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
                Some(json!({
                    "subagent_key": child.subagent_key,
                    "spawn_mode": child.spawn_mode.as_str(),
                    "thread_id": record.locator.thread_id,
                    "available": record.snapshot.state.lifecycle.initialized,
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
