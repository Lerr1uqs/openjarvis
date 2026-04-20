//! Toolset registration and tool handlers for the Obsidian-backed `obswiki` vault.

use super::runtime::{ObswikiRuntime, ObswikiRuntimeConfig, parse_obswiki_update_instruction};
use crate::agent::tool::{parse_tool_arguments, tool_definition_from_args};
use crate::agent::{
    AgentWorker, SubagentRequest, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    ToolRegistry, ToolsetCatalogEntry, ToolsetRuntime,
};
use crate::cli::InternalObswikiCommand;
use crate::config::AppConfig;
use crate::model::{IncomingMessage, ReplyTarget};
use crate::session::{SessionManager, ThreadLocator};
use crate::thread::{SubagentSpawnMode, ThreadAgentKind};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tracing::info;
use uuid::Uuid;

const OBSWIKI_TOOLSET_NAME: &str = "obswiki";

/// Thread-scoped runtime object shared by all `obswiki` tools.
pub struct ObswikiToolsetRuntime {
    runtime: Arc<ObswikiRuntime>,
}

impl ObswikiToolsetRuntime {
    /// Create one runtime holder from the provided `obswiki` config snapshot.
    pub fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }

    /// Return the shared runtime used by all `obswiki` tools.
    pub fn runtime(&self) -> Arc<ObswikiRuntime> {
        Arc::clone(&self.runtime)
    }
}

#[async_trait]
impl ToolsetRuntime for ObswikiToolsetRuntime {}

/// Register the `obswiki` toolset with one validated runtime config snapshot.
pub async fn register_obswiki_toolset_with_config(
    registry: &ToolRegistry,
    config: ObswikiRuntimeConfig,
) -> Result<()> {
    if registry.toolset_registered(OBSWIKI_TOOLSET_NAME).await {
        return Ok(());
    }

    let runtime = Arc::new(ObswikiRuntime::new(config));
    let bootstrap_created = runtime.ensure_default_workspace_vault(registry.workspace_root())?;
    let preflight = runtime.preflight()?;
    info!(
        vault_path = %preflight.vault_path.display(),
        bootstrap_created,
        qmd_configured = preflight.qmd_configured,
        qmd_cli_available = preflight.qmd_cli_available,
        "registering obswiki toolset after successful preflight"
    );

    registry
        .remember_obswiki_runtime(Arc::clone(&runtime))
        .await;
    registry
        .register_toolset_with_runtime(
            ToolsetCatalogEntry::new(
                OBSWIKI_TOOLSET_NAME,
                "Obsidian vault knowledge tools backed by Obsidian CLI and optional QMD lexical search.",
            ),
            vec![
                Arc::new(ObswikiImportRawTool::new(Arc::clone(&runtime))),
                Arc::new(ObswikiSearchTool::new(Arc::clone(&runtime))),
                Arc::new(ObswikiReadTool::new(Arc::clone(&runtime))),
                Arc::new(ObswikiWriteTool::new(Arc::clone(&runtime))),
                Arc::new(ObswikiUpdateTool::new(runtime.clone())),
            ],
            Some(Arc::new(ObswikiToolsetRuntime::new(runtime))),
        )
        .await
}

/// Run one hidden internal `obswiki` helper command.
pub async fn run_internal_obswiki_command(command: &InternalObswikiCommand) -> Result<()> {
    match command {
        InternalObswikiCommand::Prompt { content } => run_internal_obswiki_prompt(content).await,
    }
}

#[derive(Clone)]
struct ObswikiImportRawTool {
    runtime: Arc<ObswikiRuntime>,
}

#[derive(Clone)]
struct ObswikiSearchTool {
    runtime: Arc<ObswikiRuntime>,
}

#[derive(Clone)]
struct ObswikiReadTool {
    runtime: Arc<ObswikiRuntime>,
}

#[derive(Clone)]
struct ObswikiWriteTool {
    runtime: Arc<ObswikiRuntime>,
}

#[derive(Clone)]
struct ObswikiUpdateTool {
    runtime: Arc<ObswikiRuntime>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObswikiImportRawArguments {
    source_path: String,
    title: Option<String>,
    source_uri: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObswikiSearchArguments {
    query: String,
    scope: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObswikiReadArguments {
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObswikiWriteArguments {
    path: String,
    title: String,
    content: String,
    page_type: Option<String>,
    links: Option<Vec<String>>,
    source_refs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "Update one existing `wiki/` or `schema/` markdown page. `instructions` accepts one YAML object such as `operation: replace_one`, `operation: append`, `operation: prepend`, or `operation: replace_all`; when the string is not structured YAML, the whole string is treated as the new full body content."
)]
struct ObswikiUpdateArguments {
    path: String,
    instructions: String,
    expected_links: Option<Vec<String>>,
    source_refs: Option<Vec<String>>,
}

impl ObswikiImportRawTool {
    fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }
}

impl ObswikiSearchTool {
    fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }
}

impl ObswikiReadTool {
    fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }
}

impl ObswikiWriteTool {
    fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }
}

impl ObswikiUpdateTool {
    fn new(runtime: Arc<ObswikiRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl ToolHandler for ObswikiImportRawTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ObswikiImportRawArguments>(
            "obswiki_import_raw",
            "Import one external markdown file into the immutable `raw/` layer. This is the only way to ingest new Raw sources; later writes must not target `raw/`.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ObswikiImportRawArguments = parse_tool_arguments(request, "obswiki_import_raw")?;
        let document = self
            .runtime
            .import_raw_markdown(
                &PathBuf::from(&args.source_path),
                args.title.as_deref(),
                args.source_uri.as_deref(),
            )
            .await?;
        build_success_result(
            "obswiki_import_raw",
            &document.path,
            serde_json::to_value(&document)
                .context("failed to serialize obswiki_import_raw result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for ObswikiSearchTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ObswikiSearchArguments>(
            "obswiki_search",
            "Search structured candidates from the Obsidian vault. QMD lexical search is preferred when configured and available; otherwise it falls back to Obsidian CLI search. Search results are only candidates, not the final answer.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ObswikiSearchArguments = parse_tool_arguments(request, "obswiki_search")?;
        let response = self
            .runtime
            .search(&args.query, args.scope.as_deref(), args.limit.unwrap_or(10))
            .await?;
        build_success_result(
            "obswiki_search",
            "",
            serde_json::to_value(&response).context("failed to serialize obswiki_search result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for ObswikiReadTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ObswikiReadArguments>(
            "obswiki_read",
            "Read one markdown note from the Obsidian vault by explicit relative path and return parsed metadata plus the note body.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ObswikiReadArguments = parse_tool_arguments(request, "obswiki_read")?;
        let document = self.runtime.read_document(&args.path).await?;
        build_success_result(
            "obswiki_read",
            &document.path,
            serde_json::to_value(&document).context("failed to serialize obswiki_read result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for ObswikiWriteTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ObswikiWriteArguments>(
            "obswiki_write",
            "Create or fully overwrite one `wiki/` or `schema/` markdown page. Never target `raw/`; index refresh runs automatically after every successful write.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ObswikiWriteArguments = parse_tool_arguments(request, "obswiki_write")?;
        let document = self
            .runtime
            .write_document(
                &args.path,
                &args.title,
                &args.content,
                args.page_type.as_deref(),
                args.links.as_deref(),
                args.source_refs.as_deref(),
            )
            .await?;
        build_success_result(
            "obswiki_write",
            &document.path,
            serde_json::to_value(&document).context("failed to serialize obswiki_write result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for ObswikiUpdateTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<ObswikiUpdateArguments>(
            "obswiki_update",
            "Update one existing `wiki/` or `schema/` page with deterministic YAML instructions. Supported operations are `replace_all`, `replace_one`, `append`, and `prepend`; unstructured text falls back to full-body replacement.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: ObswikiUpdateArguments = parse_tool_arguments(request, "obswiki_update")?;
        let instruction = parse_obswiki_update_instruction(&args.instructions)?;
        let document = self
            .runtime
            .update_document(
                &args.path,
                instruction,
                args.expected_links.as_deref(),
                args.source_refs.as_deref(),
            )
            .await?;
        build_success_result(
            "obswiki_update",
            &document.path,
            serde_json::to_value(&document).context("failed to serialize obswiki_update result")?,
        )
    }
}

fn build_success_result(tool_name: &str, path: &str, payload: Value) -> Result<ToolCallResult> {
    Ok(ToolCallResult {
        content: if path.is_empty() {
            format!("{tool_name} completed successfully")
        } else {
            format!("{tool_name} completed successfully for `{path}`")
        },
        metadata: json!({
            "event_kind": tool_name,
            "path": path,
            "payload": payload,
        }),
        is_error: false,
    })
}

async fn run_internal_obswiki_prompt(content: &str) -> Result<()> {
    let config = AppConfig::load()?;
    let agent = AgentWorker::from_config(&config).await?;
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(agent.thread_runtime());

    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_internal_obswiki".to_string()),
        channel: "internal_obswiki".to_string(),
        user_id: "internal_obswiki".to_string(),
        user_name: Some("internal_obswiki".to_string()),
        content: content.to_string(),
        external_thread_id: Some("internal_obswiki_prompt".to_string()),
        received_at: Utc::now(),
        metadata: json!({
            "internal_obswiki": true,
        }),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "internal_obswiki".to_string(),
            receive_id_type: "internal_obswiki".to_string(),
        },
    };
    let parent_locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .context("failed to create internal obswiki parent thread")?;
    let child_locator =
        ThreadLocator::for_child(&parent_locator, "obswiki", SubagentSpawnMode::Persist);
    let child_locator = sessions
        .create_thread_at(
            &child_locator,
            incoming.received_at,
            ThreadAgentKind::Obswiki,
        )
        .await
        .context("failed to create internal obswiki child thread")?;
    let result = agent
        .subagent_runner()
        .run(SubagentRequest {
            parent_locator,
            child_locator,
            prompt: content.to_string(),
            sessions,
        })
        .await
        .context("failed to execute internal obswiki child thread")?;
    println!("{}", result.output.reply);
    Ok(())
}
