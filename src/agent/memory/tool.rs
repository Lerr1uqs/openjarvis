//! Loadable `memory` toolset backed by the local [`MemoryRepository`].

use super::repository::{MemoryRepository, MemoryType, MemoryWriteRequest};
use super::search::MemorySearchService;
use crate::agent::tool::{parse_tool_arguments, tool_definition_from_args};
use crate::agent::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry, ToolsetCatalogEntry,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::info;

const MEMORY_TOOLSET_NAME: &str = "memory";

/// Register the thread-loadable `memory` toolset.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use openjarvis::{
///     agent::{ToolRegistry, memory::register_memory_toolset},
/// };
///
/// let registry = ToolRegistry::new();
/// register_memory_toolset(&registry).await?;
/// # Ok(())
/// # }
/// ```
pub async fn register_memory_toolset(registry: &ToolRegistry) -> Result<()> {
    if registry.toolset_registered(MEMORY_TOOLSET_NAME).await {
        return Ok(());
    }

    let repository = registry.memory_repository();
    let search_service = Arc::new(MemorySearchService::new(
        registry.memory_search_config().clone(),
    )?);
    registry
        .register_toolset(
            ToolsetCatalogEntry::new(
                MEMORY_TOOLSET_NAME,
                "Local active/passive memory tools backed by ./.openjarvis/memory markdown files.",
            ),
            vec![
                Arc::new(MemoryGetTool::new(Arc::clone(&repository))),
                Arc::new(MemorySearchTool::new(
                    Arc::clone(&repository),
                    search_service,
                )),
                Arc::new(MemoryWriteTool::new(Arc::clone(&repository))),
                Arc::new(MemoryListTool::new(repository)),
            ],
        )
        .await
}

#[derive(Clone)]
struct MemoryGetTool {
    repository: Arc<MemoryRepository>,
}

#[derive(Clone)]
struct MemorySearchTool {
    repository: Arc<MemoryRepository>,
    service: Arc<MemorySearchService>,
}

#[derive(Clone)]
struct MemoryWriteTool {
    repository: Arc<MemoryRepository>,
}

#[derive(Clone)]
struct MemoryListTool {
    repository: Arc<MemoryRepository>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryGetArguments {
    path: String,
    #[serde(rename = "type")]
    memory_type: MemoryType,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemorySearchArguments {
    query: String,
    #[serde(rename = "type")]
    memory_type: Option<MemoryType>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "Write one memory markdown document. Defaults to `passive`. For `active`, `keywords` must be explicit, highly specific names directly provided by the user. Do not invent broad synonyms, preferences, traits, or related concepts. If the user did not explicitly provide suitable active keywords, ask first instead of calling this tool."
)]
struct MemoryWriteArguments {
    path: String,
    title: String,
    content: String,
    #[serde(rename = "type")]
    memory_type: Option<MemoryType>,
    #[schemars(
        description = "Only used for `active` memory. Must contain explicit, highly specific names directly provided by the user, such as an exact person, org, project, repo, or product name. Do not invent extra keywords, broad aliases, traits, or inferred concepts. If the user did not explicitly provide such names, ask first instead of writing active memory."
    )]
    keywords: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryListArguments {
    #[serde(rename = "type")]
    memory_type: Option<MemoryType>,
    dir: Option<String>,
}

impl MemoryGetTool {
    fn new(repository: Arc<MemoryRepository>) -> Self {
        Self { repository }
    }
}

impl MemorySearchTool {
    fn new(repository: Arc<MemoryRepository>, service: Arc<MemorySearchService>) -> Self {
        Self {
            repository,
            service,
        }
    }
}

impl MemoryWriteTool {
    fn new(repository: Arc<MemoryRepository>) -> Self {
        Self { repository }
    }
}

impl MemoryListTool {
    fn new(repository: Arc<MemoryRepository>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl ToolHandler for MemoryGetTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<MemoryGetArguments>(
            "memory_get",
            "Read one memory markdown document by explicit `type + path` and return metadata plus body content.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: MemoryGetArguments = parse_tool_arguments(request, "memory_get")?;
        let document = self.repository.get(args.memory_type, &args.path)?;
        info!(
            memory_type = document.memory_type.as_dir_name(),
            path = %document.path,
            "returned memory document contents"
        );
        build_success_result(
            "memory_get",
            &document.path,
            serde_json::to_value(&document).context("failed to serialize memory_get result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for MemorySearchTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<MemorySearchArguments>(
            "memory_search",
            "Search memory titles, keywords, paths, and bodies. Returns structured candidates only, not full document bodies.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: MemorySearchArguments = parse_tool_arguments(request, "memory_search")?;
        let response = self
            .service
            .search(
                &self.repository,
                &args.query,
                args.memory_type,
                args.limit.unwrap_or(10),
            )
            .await?;
        info!(
            query = %response.query,
            total_matches = response.total_matches,
            "searched local memory repository"
        );
        build_success_result(
            "memory_search",
            "",
            serde_json::to_value(&response).context("failed to serialize memory_search result")?,
        )
    }
}

#[async_trait]
impl ToolHandler for MemoryWriteTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<MemoryWriteArguments>(
            "memory_write",
            "Write one memory markdown document. Defaults to `passive`; `active` writes must include non-empty `keywords`, and those keywords must be explicit, highly specific names directly provided by the user. Do not invent extra keywords; if the user did not specify them, ask first.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: MemoryWriteArguments = parse_tool_arguments(request, "memory_write")?;
        let document = self.repository.write(MemoryWriteRequest {
            memory_type: args.memory_type.unwrap_or(MemoryType::Passive),
            path: args.path,
            title: args.title,
            content: args.content,
            keywords: args.keywords,
        })?;
        info!(
            memory_type = document.memory_type.as_dir_name(),
            path = %document.path,
            "persisted memory document from tool call"
        );
        build_success_result(
            "memory_write",
            &document.path,
            json!({
                "type": document.memory_type,
                "path": document.path,
                "title": document.metadata.title,
                "created_at": document.metadata.created_at,
                "updated_at": document.metadata.updated_at,
                "keywords": document.metadata.keywords,
            }),
        )
    }
}

#[async_trait]
impl ToolHandler for MemoryListTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<MemoryListArguments>(
            "memory_list",
            "List structured memory candidates by optional `type` and optional directory prefix. Does not return full document bodies.",
        )
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let args: MemoryListArguments = parse_tool_arguments(request, "memory_list")?;
        let items = self
            .repository
            .list(args.memory_type, args.dir.as_deref())?;
        info!(item_count = items.len(), "listed local memory candidates");
        build_success_result(
            "memory_list",
            args.dir.as_deref().unwrap_or_default(),
            json!({
                "type": args.memory_type,
                "dir": args.dir,
                "items": items,
            }),
        )
    }
}

fn build_success_result(tool_name: &str, path: &str, payload: Value) -> Result<ToolCallResult> {
    Ok(ToolCallResult {
        content: serialize_pretty_json(&payload)?,
        metadata: json!({
            "toolset": MEMORY_TOOLSET_NAME,
            "event_kind": tool_name,
            "path": path,
            "result": payload,
        }),
        is_error: false,
    })
}

fn serialize_pretty_json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string_pretty(value).context("failed to render structured memory tool output")
}
