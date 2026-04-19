//! Browser toolset registration, tool handlers, and internal helper commands.

use super::super::{parse_tool_arguments, tool_definition_from_args};
use super::{
    default_sidecar_script_path,
    protocol::{
        BrowserActionResult, BrowserCloseResult, BrowserConsoleEntry, BrowserConsoleLevel,
        BrowserConsoleResult, BrowserDiagnosticsQuery, BrowserErrorEntry, BrowserErrorKind,
        BrowserErrorsResult, BrowserNavigateResult, BrowserOpenRequest, BrowserOpenResult,
        BrowserRequestDiagnosticsQuery, BrowserRequestEntry, BrowserRequestResultKind,
        BrowserRequestsResult, BrowserScreenshotResult, BrowserSessionMode, BrowserSidecarRequest,
        BrowserSidecarRequestPayload, BrowserSidecarResponse, BrowserSidecarResponsePayload,
        BrowserSnapshotElement, BrowserSnapshotResult, BrowserTypeResult,
    },
    service::{BrowserProcessCommandSpec, BrowserRuntimeOptions},
    session::{BrowserSessionCloseOutcome, BrowserSessionManager, BrowserSessionManagerConfig},
};
use crate::{
    agent::{
        ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        ToolRegistry, ToolsetCatalogEntry, ToolsetRuntime, empty_tool_input_schema,
    },
    cli::{InternalBrowserCommand, InternalBrowserMode},
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Thread-scoped runtime object that owns browser sessions for the `browser` toolset.
pub struct BrowserToolsetRuntime {
    sessions: Arc<BrowserSessionManager>,
}

impl BrowserToolsetRuntime {
    /// Create one browser toolset runtime from the provided session manager config.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserToolsetRuntime;
    ///
    /// let runtime = BrowserToolsetRuntime::new(Default::default());
    /// assert!(!runtime.session_manager().has_session_blocking("thread-1"));
    /// ```
    pub fn new(config: BrowserSessionManagerConfig) -> Self {
        Self {
            sessions: Arc::new(BrowserSessionManager::new(config)),
        }
    }

    /// Return the shared browser session manager used by all browser tools.
    pub fn session_manager(&self) -> Arc<BrowserSessionManager> {
        Arc::clone(&self.sessions)
    }
}

#[async_trait]
impl ToolsetRuntime for BrowserToolsetRuntime {
    async fn on_unload(&self, thread_id: &str) -> Result<()> {
        let _ = self.sessions.close(thread_id).await?;
        Ok(())
    }
}

/// Register the default `browser` toolset powered by the bundled Node sidecar.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use openjarvis::agent::{ToolRegistry, tool::browser::register_browser_toolset};
///
/// let registry = ToolRegistry::new();
/// register_browser_toolset(&registry).await?;
/// # Ok(())
/// # }
/// ```
pub async fn register_browser_toolset(registry: &ToolRegistry) -> Result<()> {
    register_browser_toolset_with_config(registry, BrowserSessionManagerConfig::default()).await
}

/// Register the `browser` toolset with an explicit browser session manager config.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use openjarvis::agent::{ToolRegistry, tool::browser::register_browser_toolset_with_config};
///
/// let registry = ToolRegistry::new();
/// register_browser_toolset_with_config(&registry, Default::default()).await?;
/// # Ok(())
/// # }
/// ```
pub async fn register_browser_toolset_with_config(
    registry: &ToolRegistry,
    config: BrowserSessionManagerConfig,
) -> Result<()> {
    if registry.toolset_registered("browser").await {
        return Ok(());
    }

    let runtime = Arc::new(BrowserToolsetRuntime::new(config));
    let sessions = runtime.session_manager();
    registry
        .remember_browser_session_manager(Arc::clone(&sessions))
        .await;
    registry
        .register_toolset_with_runtime(
            ToolsetCatalogEntry::new(
                "browser",
                "Browser automation tools powered by a Node Playwright sidecar.",
            ),
            vec![
                Arc::new(BrowserOpenTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserNavigateTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserConsoleTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserErrorsTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserRequestsTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserSnapshotTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserClickRefTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserClickMatchTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserTypeRefTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserTypeMatchTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserScreenshotTool::new(Arc::clone(&sessions))),
                Arc::new(BrowserCloseTool::new(Arc::clone(&sessions))),
            ],
            Some(runtime),
        )
        .await
}

/// Run one hidden internal browser helper command.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use openjarvis::agent::tool::browser::run_internal_browser_command;
/// use openjarvis::cli::InternalBrowserCommand;
///
/// run_internal_browser_command(&InternalBrowserCommand::MockSidecar).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_internal_browser_command(command: &InternalBrowserCommand) -> Result<()> {
    match command {
        InternalBrowserCommand::Smoke {
            url,
            mode,
            cdp_endpoint,
            headless,
            output_dir,
            node_bin,
            script_path,
            chrome_path,
            cookies_state_file,
            load_cookies_on_open,
            save_cookies_on_close,
        } => {
            let manager = BrowserSessionManager::new(build_helper_manager_config(
                *headless,
                output_dir.clone(),
                node_bin.clone(),
                script_path.clone(),
                chrome_path.clone(),
                cookies_state_file.clone(),
                *load_cookies_on_open,
                *save_cookies_on_close,
                "openjarvis-browser-smoke",
            ));
            run_smoke_flow(
                &manager,
                helper_open_request(*mode, cdp_endpoint.clone())?,
                url,
            )
            .await
        }
        InternalBrowserCommand::Script {
            steps_file,
            mode,
            cdp_endpoint,
            headless,
            output_dir,
            node_bin,
            script_path,
            chrome_path,
            cookies_state_file,
            load_cookies_on_open,
            save_cookies_on_close,
        } => {
            let manager = BrowserSessionManager::new(build_helper_manager_config(
                *headless,
                output_dir.clone(),
                node_bin.clone(),
                script_path.clone(),
                chrome_path.clone(),
                cookies_state_file.clone(),
                *load_cookies_on_open,
                *save_cookies_on_close,
                "openjarvis-browser-script",
            ));
            run_script_flow(
                &manager,
                steps_file,
                helper_open_request(*mode, cdp_endpoint.clone())?,
            )
            .await
        }
        InternalBrowserCommand::MockSidecar => run_mock_sidecar().await,
    }
}

#[derive(Clone)]
struct BrowserToolBase {
    sessions: Arc<BrowserSessionManager>,
}

impl BrowserToolBase {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self { sessions }
    }

    fn require_thread_id<'a>(
        &self,
        context: &'a ToolCallContext,
        tool_name: &str,
    ) -> Result<&'a str> {
        context
            .thread_id()
            .with_context(|| format!("browser tool `{tool_name}` requires thread context"))
    }
}

struct BrowserNavigateTool {
    base: BrowserToolBase,
}

struct BrowserOpenTool {
    base: BrowserToolBase,
}

struct BrowserConsoleTool {
    base: BrowserToolBase,
}

struct BrowserErrorsTool {
    base: BrowserToolBase,
}

struct BrowserRequestsTool {
    base: BrowserToolBase,
}

struct BrowserSnapshotTool {
    base: BrowserToolBase,
}

struct BrowserClickRefTool {
    base: BrowserToolBase,
}

struct BrowserClickMatchTool {
    base: BrowserToolBase,
}

struct BrowserTypeRefTool {
    base: BrowserToolBase,
}

struct BrowserTypeMatchTool {
    base: BrowserToolBase,
}

struct BrowserScreenshotTool {
    base: BrowserToolBase,
}

struct BrowserCloseTool {
    base: BrowserToolBase,
}

impl BrowserNavigateTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserOpenTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserConsoleTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserErrorsTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserRequestsTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserSnapshotTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserClickRefTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserClickMatchTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserTypeRefTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserTypeMatchTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserScreenshotTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

impl BrowserCloseTool {
    fn new(sessions: Arc<BrowserSessionManager>) -> Self {
        Self {
            base: BrowserToolBase::new(sessions),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserOpenArguments {
    mode: Option<BrowserSessionMode>,
    cdp_endpoint: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserNavigateArguments {
    url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserRefArguments {
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserSnapshotArguments {
    /// Optional upper bound for how many interactive elements the sidecar should include.
    max_elements: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserTypeArguments {
    #[serde(rename = "ref")]
    reference: String,
    text: String,
    submit: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserElementMatchArguments {
    role: Option<String>,
    tag_name: Option<String>,
    label_contains: Option<String>,
    text_contains: Option<String>,
    href_contains: Option<String>,
    placeholder_contains: Option<String>,
    input_type: Option<String>,
    section_hint: Option<String>,
    disabled: Option<bool>,
    nth: Option<usize>,
    max_elements: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserTypeMatchArguments {
    #[serde(flatten)]
    matcher: BrowserElementMatchArguments,
    text: String,
    submit: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserScreenshotArguments {
    path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrowserNoArguments {}

#[async_trait]
impl ToolHandler for BrowserOpenTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserOpenArguments>(
            "browser__open",
            "Open or replace the current thread-scoped browser session. Use `mode=attach` with `cdp_endpoint` to connect to an existing Chromium instance.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__open` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__open")?;
        let args: BrowserOpenArguments = parse_tool_arguments(request, "browser__open")?;
        let request = BrowserOpenRequest {
            mode: args.mode.unwrap_or(BrowserSessionMode::Launch),
            cdp_endpoint: args.cdp_endpoint,
        };
        let result = self.base.sessions.open(thread_id, request).await?;
        Ok(render_open_result(result))
    }
}

#[async_trait]
impl ToolHandler for BrowserNavigateTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserNavigateArguments>(
            "browser__navigate",
            "Open one URL inside the current thread-scoped browser session and return a lightweight snapshot for the next action.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__navigate` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__navigate")?;
        let args: BrowserNavigateArguments = parse_tool_arguments(request, "browser__navigate")?;
        let result = self.base.sessions.navigate(thread_id, &args.url).await?;
        // Bug fix: include an immediate page snapshot after navigation so the model can observe
        // refs and page structure instead of repeatedly navigating to the same URL.
        let snapshot = self
            .base
            .sessions
            .snapshot(thread_id, Some(NAVIGATE_SNAPSHOT_MAX_ELEMENTS))
            .await?;
        Ok(render_navigate_result(result, snapshot))
    }
}

#[async_trait]
impl ToolHandler for BrowserConsoleTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserDiagnosticsQuery>(
            "browser__console",
            "Return recent console diagnostics from the current thread-scoped browser session.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__console` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__console")?;
        let query: BrowserDiagnosticsQuery = parse_tool_arguments(request, "browser__console")?;
        let result = self.base.sessions.console(thread_id, query.clone()).await?;
        Ok(render_console_result(query, result))
    }
}

#[async_trait]
impl ToolHandler for BrowserErrorsTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserDiagnosticsQuery>(
            "browser__errors",
            "Return recent page errors and request failures from the current thread-scoped browser session.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__errors` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__errors")?;
        let query: BrowserDiagnosticsQuery = parse_tool_arguments(request, "browser__errors")?;
        let result = self.base.sessions.errors(thread_id, query.clone()).await?;
        Ok(render_errors_result(query, result))
    }
}

#[async_trait]
impl ToolHandler for BrowserRequestsTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserRequestDiagnosticsQuery>(
            "browser__requests",
            "Return recent network request summaries from the current thread-scoped browser session.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__requests` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__requests")?;
        let query: BrowserRequestDiagnosticsQuery =
            parse_tool_arguments(request, "browser__requests")?;
        let result = self
            .base
            .sessions
            .requests(thread_id, query.clone())
            .await?;
        Ok(render_requests_result(query, result))
    }
}

#[async_trait]
impl ToolHandler for BrowserSnapshotTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserSnapshotArguments>(
            "browser__snapshot",
            "Capture a text snapshot and element refs from the current browser page. `max_elements` can raise the observation cap when a page has many interactive items.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__snapshot` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__snapshot")?;
        let args: BrowserSnapshotArguments = parse_tool_arguments(request, "browser__snapshot")?;
        let result = self
            .base
            .sessions
            .snapshot(thread_id, args.max_elements)
            .await?;
        Ok(render_snapshot_result(result))
    }
}

#[async_trait]
impl ToolHandler for BrowserClickRefTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserRefArguments>(
            "browser__click_ref",
            "Click one previously observed browser snapshot ref inside the current thread.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__click_ref` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self
            .base
            .require_thread_id(&context, "browser__click_ref")?;
        let args: BrowserRefArguments = parse_tool_arguments(request, "browser__click_ref")?;
        let result = self
            .base
            .sessions
            .click_ref(thread_id, &args.reference)
            .await?;
        Ok(render_click_result(result))
    }
}

#[async_trait]
impl ToolHandler for BrowserClickMatchTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserElementMatchArguments>(
            "browser__click_match",
            "Capture a fresh snapshot, resolve one element by semantic match fields, and click the matched ref. Use this when raw refs are too unstable on dynamic pages.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__click_match` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self
            .base
            .require_thread_id(&context, "browser__click_match")?;
        let args: BrowserElementMatchArguments =
            parse_tool_arguments(request, "browser__click_match")?;
        let matched = resolve_element_match(&self.base.sessions, thread_id, &args).await?;
        let result = self
            .base
            .sessions
            .click_ref(thread_id, &matched.reference)
            .await?;
        Ok(render_click_match_result(result, matched, &args))
    }
}

#[async_trait]
impl ToolHandler for BrowserTypeRefTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserTypeArguments>(
            "browser__type_ref",
            "Type text into one previously observed browser snapshot ref.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__type_ref` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__type_ref")?;
        let args: BrowserTypeArguments = parse_tool_arguments(request, "browser__type_ref")?;
        let result = self
            .base
            .sessions
            .type_ref(
                thread_id,
                &args.reference,
                &args.text,
                args.submit.unwrap_or(false),
            )
            .await?;
        Ok(render_type_result(result))
    }
}

#[async_trait]
impl ToolHandler for BrowserTypeMatchTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserTypeMatchArguments>(
            "browser__type_match",
            "Capture a fresh snapshot, resolve one element by semantic match fields, and type into the matched ref.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__type_match` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self
            .base
            .require_thread_id(&context, "browser__type_match")?;
        let args: BrowserTypeMatchArguments = parse_tool_arguments(request, "browser__type_match")?;
        let matched = resolve_element_match(&self.base.sessions, thread_id, &args.matcher).await?;
        let result = self
            .base
            .sessions
            .type_ref(
                thread_id,
                &matched.reference,
                &args.text,
                args.submit.unwrap_or(false),
            )
            .await?;
        Ok(render_type_match_result(result, matched, &args.matcher))
    }
}

#[async_trait]
impl ToolHandler for BrowserScreenshotTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserScreenshotArguments>(
            "browser__screenshot",
            "Write a screenshot for the current browser page and return the saved file path.",
        )
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__screenshot` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self
            .base
            .require_thread_id(&context, "browser__screenshot")?;
        let args: BrowserScreenshotArguments =
            parse_tool_arguments(request, "browser__screenshot")?;
        let requested_path = args.path.as_deref().map(Path::new);
        let result = self
            .base
            .sessions
            .screenshot(thread_id, requested_path)
            .await?;
        Ok(render_screenshot_result(result))
    }
}

#[async_trait]
impl ToolHandler for BrowserCloseTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "browser__close".to_string(),
            description: "Close the current thread-scoped browser session and release resources."
                .to_string(),
            input_schema: empty_tool_input_schema(),
            source: crate::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        bail!("browser tool `browser__close` requires thread context");
    }

    async fn call_with_context(
        &self,
        context: ToolCallContext,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.base.require_thread_id(&context, "browser__close")?;
        let _: BrowserNoArguments = parse_tool_arguments(request, "browser__close")?;
        let result = self.base.sessions.close(thread_id).await?;
        Ok(render_close_result(result))
    }
}

fn render_navigate_result(
    result: BrowserNavigateResult,
    snapshot: BrowserSnapshotResult,
) -> ToolCallResult {
    ToolCallResult {
        content: format!(
            "Navigated to {}.\nCurrent page snapshot:\n{}",
            result.url, snapshot.snapshot_text
        ),
        metadata: json!({
            "toolset": "browser",
            "url": result.url,
            "title": result.title,
            "snapshot": {
                "url": snapshot.url,
                "title": snapshot.title,
                "elements": snapshot.elements,
                "element_count": snapshot.elements.len(),
                "total_candidate_count": snapshot.total_candidate_count,
                "truncated": snapshot.truncated,
            },
        }),
        is_error: false,
    }
}

fn render_open_result(result: BrowserOpenResult) -> ToolCallResult {
    ToolCallResult {
        content: format!(
            "Opened browser session in `{}` mode at {}.",
            match result.mode {
                BrowserSessionMode::Launch => "launch",
                BrowserSessionMode::Attach => "attach",
            },
            result.url
        ),
        metadata: json!({
            "toolset": "browser",
            "mode": result.mode,
            "url": result.url,
            "title": result.title,
            "cookies_loaded": result.cookies_loaded,
        }),
        is_error: false,
    }
}

fn render_console_result(
    query: BrowserDiagnosticsQuery,
    result: BrowserConsoleResult,
) -> ToolCallResult {
    ToolCallResult {
        content: render_console_entries(&result.entries),
        metadata: json!({
            "toolset": "browser",
            "limit": query.limit,
            "entries": result.entries,
            "entry_count": result.entries.len(),
        }),
        is_error: false,
    }
}

fn render_errors_result(
    query: BrowserDiagnosticsQuery,
    result: BrowserErrorsResult,
) -> ToolCallResult {
    ToolCallResult {
        content: render_error_entries(&result.entries),
        metadata: json!({
            "toolset": "browser",
            "limit": query.limit,
            "entries": result.entries,
            "entry_count": result.entries.len(),
        }),
        is_error: false,
    }
}

fn render_requests_result(
    query: BrowserRequestDiagnosticsQuery,
    result: BrowserRequestsResult,
) -> ToolCallResult {
    ToolCallResult {
        content: render_request_entries(&result.entries, query.failed_only),
        metadata: json!({
            "toolset": "browser",
            "limit": query.limit,
            "failed_only": query.failed_only,
            "entries": result.entries,
            "entry_count": result.entries.len(),
        }),
        is_error: false,
    }
}

fn render_snapshot_result(result: BrowserSnapshotResult) -> ToolCallResult {
    ToolCallResult {
        content: result.snapshot_text.clone(),
        metadata: json!({
            "toolset": "browser",
            "url": result.url,
            "title": result.title,
            "elements": result.elements,
            "element_count": result.elements.len(),
            "total_candidate_count": result.total_candidate_count,
            "truncated": result.truncated,
        }),
        is_error: false,
    }
}

fn render_console_entries(entries: &[BrowserConsoleEntry]) -> String {
    if entries.is_empty() {
        return "No browser console records in the current session.".to_string();
    }

    entries
        .iter()
        .map(|entry| {
            let mut line = format!(
                "[{}][{}] {} ({})",
                entry.timestamp,
                render_console_level(entry.level),
                entry.text,
                entry.page_url
            );
            if let Some(location) = &entry.location {
                line.push_str(&format!(
                    " @ {}:{}:{}",
                    location.url, location.line_number, location.column_number
                ));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_error_entries(entries: &[BrowserErrorEntry]) -> String {
    if entries.is_empty() {
        return "No browser error records in the current session.".to_string();
    }

    entries
        .iter()
        .map(|entry| {
            let mut line = format!(
                "[{}][{}] {}",
                entry.timestamp,
                render_error_kind(entry.kind),
                entry.message
            );
            if let Some(page_url) = &entry.page_url {
                line.push_str(&format!(" page={page_url}"));
            }
            if let Some(request_url) = &entry.request_url {
                line.push_str(&format!(" request={request_url}"));
            }
            if let Some(reason) = &entry.reason {
                line.push_str(&format!(" reason={reason}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_request_entries(entries: &[BrowserRequestEntry], failed_only: bool) -> String {
    if entries.is_empty() {
        return if failed_only {
            "No failed browser requests in the current session.".to_string()
        } else {
            "No browser request records in the current session.".to_string()
        };
    }

    entries
        .iter()
        .map(|entry| {
            let mut line = format!(
                "[{}][{}] {} {} ({})",
                entry.timestamp,
                render_request_result(entry.result),
                entry.method,
                entry.url,
                entry.resource_type
            );
            if let Some(status) = entry.status {
                line.push_str(&format!(" status={status}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_console_level(level: BrowserConsoleLevel) -> &'static str {
    match level {
        BrowserConsoleLevel::Log => "log",
        BrowserConsoleLevel::Info => "info",
        BrowserConsoleLevel::Warn => "warn",
        BrowserConsoleLevel::Error => "error",
        BrowserConsoleLevel::Debug => "debug",
    }
}

fn render_error_kind(kind: BrowserErrorKind) -> &'static str {
    match kind {
        BrowserErrorKind::PageError => "page_error",
        BrowserErrorKind::RequestFailed => "request_failed",
    }
}

fn render_request_result(result: BrowserRequestResultKind) -> &'static str {
    match result {
        BrowserRequestResultKind::Pending => "pending",
        BrowserRequestResultKind::Ok => "ok",
        BrowserRequestResultKind::HttpError => "http_error",
        BrowserRequestResultKind::Failed => "failed",
    }
}

fn render_click_result(result: BrowserActionResult) -> ToolCallResult {
    ToolCallResult {
        content: format!("Clicked ref `{}`.", result.reference),
        metadata: json!({
            "toolset": "browser",
            "ref": result.reference,
            "url": result.url,
            "title": result.title,
            "opened_new_page": result.opened_new_page,
        }),
        is_error: false,
    }
}

fn render_type_result(result: BrowserTypeResult) -> ToolCallResult {
    ToolCallResult {
        content: format!(
            "Typed {} characters into ref `{}`.",
            result.text_length, result.reference
        ),
        metadata: json!({
            "toolset": "browser",
            "ref": result.reference,
            "url": result.url,
            "title": result.title,
            "text_length": result.text_length,
            "submitted": result.submitted,
            "opened_new_page": result.opened_new_page,
        }),
        is_error: false,
    }
}

fn render_click_match_result(
    result: BrowserActionResult,
    matched: BrowserSnapshotElement,
    matcher: &BrowserElementMatchArguments,
) -> ToolCallResult {
    ToolCallResult {
        content: format!(
            "Clicked matched ref `{}` ({}/{}).",
            result.reference, matched.role, matched.tag_name
        ),
        metadata: json!({
            "toolset": "browser",
            "ref": result.reference,
            "url": result.url,
            "title": result.title,
            "opened_new_page": result.opened_new_page,
            "matched_element": matched,
            "match": render_match_metadata(matcher),
        }),
        is_error: false,
    }
}

fn render_type_match_result(
    result: BrowserTypeResult,
    matched: BrowserSnapshotElement,
    matcher: &BrowserElementMatchArguments,
) -> ToolCallResult {
    ToolCallResult {
        content: format!(
            "Typed {} characters into matched ref `{}` ({}/{}).",
            result.text_length, result.reference, matched.role, matched.tag_name
        ),
        metadata: json!({
            "toolset": "browser",
            "ref": result.reference,
            "url": result.url,
            "title": result.title,
            "text_length": result.text_length,
            "submitted": result.submitted,
            "opened_new_page": result.opened_new_page,
            "matched_element": matched,
            "match": render_match_metadata(matcher),
        }),
        is_error: false,
    }
}

fn render_screenshot_result(result: BrowserScreenshotResult) -> ToolCallResult {
    ToolCallResult {
        content: result.path.clone(),
        metadata: json!({
            "toolset": "browser",
            "url": result.url,
            "title": result.title,
            "path": result.path,
        }),
        is_error: false,
    }
}

fn render_close_result(result: BrowserSessionCloseOutcome) -> ToolCallResult {
    let artifacts_dir = result
        .artifacts
        .as_ref()
        .map(|artifacts| artifacts.session_dir.display().to_string());
    ToolCallResult {
        content: if result.had_session {
            match (&result.auto_exported_path, result.exported_cookie_count) {
                (Some(path), Some(cookie_count)) => format!(
                    "Browser session closed. Auto-exported {cookie_count} cookies to {path}."
                ),
                _ => "Browser session closed.".to_string(),
            }
        } else {
            "No browser session was active for the current thread.".to_string()
        },
        metadata: json!({
            "toolset": "browser",
            "had_session": result.had_session,
            "kept_artifacts": result.kept_artifacts,
            "artifacts_dir": artifacts_dir,
            "mode": result.session_mode,
            "auto_exported_path": result.auto_exported_path,
            "exported_cookie_count": result.exported_cookie_count,
        }),
        is_error: false,
    }
}

fn build_helper_manager_config(
    headless: bool,
    output_dir: Option<PathBuf>,
    node_bin: String,
    script_path: Option<PathBuf>,
    chrome_path: Option<PathBuf>,
    cookies_state_file: Option<PathBuf>,
    load_cookies_on_open: bool,
    save_cookies_on_close: bool,
    default_root_dir_name: &str,
) -> BrowserSessionManagerConfig {
    let artifact_root =
        output_dir.unwrap_or_else(|| std::env::temp_dir().join(default_root_dir_name));
    let script_path = script_path.unwrap_or_else(default_sidecar_script_path);
    BrowserSessionManagerConfig {
        process: BrowserProcessCommandSpec {
            executable: node_bin,
            args: vec![script_path.display().to_string()],
            env: Default::default(),
        },
        runtime: BrowserRuntimeOptions {
            headless,
            keep_artifacts: true,
            chrome_executable: chrome_path,
            cookies_state_file,
            load_cookies_on_open,
            save_cookies_on_close,
            ..Default::default()
        },
        artifact_root,
    }
}

fn helper_open_request(
    mode: InternalBrowserMode,
    cdp_endpoint: Option<String>,
) -> Result<BrowserOpenRequest> {
    match mode {
        InternalBrowserMode::Launch => {
            if cdp_endpoint.is_some() {
                bail!("launch mode does not accept `cdp_endpoint`");
            }
            Ok(BrowserOpenRequest::launch())
        }
        InternalBrowserMode::Attach => {
            let cdp_endpoint = cdp_endpoint
                .filter(|endpoint| !endpoint.trim().is_empty())
                .with_context(|| "attach mode requires a non-empty `cdp_endpoint`")?;
            Ok(BrowserOpenRequest::attach(cdp_endpoint))
        }
    }
}

async fn ensure_helper_session(
    manager: &BrowserSessionManager,
    thread_id: &str,
    open_request: &BrowserOpenRequest,
) -> Result<Option<BrowserOpenResult>> {
    if manager.has_session(thread_id).await {
        return Ok(None);
    }

    let result = manager.open(thread_id, open_request.clone()).await?;
    Ok(Some(result))
}

async fn run_smoke_flow(
    manager: &BrowserSessionManager,
    open_request: BrowserOpenRequest,
    url: &str,
) -> Result<()> {
    let thread_id = "internal-browser-smoke";
    let open = manager.open(thread_id, open_request).await?;
    let navigate = manager.navigate(thread_id, url).await?;
    let snapshot = manager.snapshot(thread_id, Some(120)).await?;
    let screenshot = manager.screenshot(thread_id, None).await?;
    let close = manager.close(thread_id).await?;

    println!(
        "open: mode={:?} url={} cookies_loaded={}",
        open.mode, open.url, open.cookies_loaded
    );
    println!("navigate: {}", navigate.url);
    println!("title: {}", navigate.title);
    println!("snapshot:");
    println!("{}", snapshot.snapshot_text);
    println!("screenshot: {}", screenshot.path);
    if let Some(artifacts) = close.artifacts {
        println!("artifacts: {}", artifacts.session_dir.display());
    }

    Ok(())
}

const DEFAULT_MATCH_MAX_ELEMENTS: usize = 500;
const NAVIGATE_SNAPSHOT_MAX_ELEMENTS: usize = 24;

fn resolve_match_limit(args: &BrowserElementMatchArguments) -> usize {
    args.max_elements
        .unwrap_or(DEFAULT_MATCH_MAX_ELEMENTS)
        .clamp(1, 500)
}

async fn resolve_element_match(
    manager: &BrowserSessionManager,
    thread_id: &str,
    args: &BrowserElementMatchArguments,
) -> Result<BrowserSnapshotElement> {
    validate_match_arguments(args)?;
    let snapshot = manager
        .snapshot(thread_id, Some(resolve_match_limit(args)))
        .await?;
    let nth = args.nth.unwrap_or(1);
    snapshot
        .elements
        .iter()
        .filter(|element| element_matches(element, args))
        .nth(nth - 1)
        .cloned()
        .with_context(|| {
            format!(
                "failed to resolve browser element match {} within {} observed elements",
                describe_matcher(args),
                snapshot.elements.len()
            )
        })
}

fn validate_match_arguments(args: &BrowserElementMatchArguments) -> Result<()> {
    let has_predicate = args.role.is_some()
        || args.tag_name.is_some()
        || args.label_contains.is_some()
        || args.text_contains.is_some()
        || args.href_contains.is_some()
        || args.placeholder_contains.is_some()
        || args.input_type.is_some()
        || args.section_hint.is_some()
        || args.disabled.is_some();
    if !has_predicate {
        bail!(
            "browser element match requires at least one predicate field such as `role`, `href_contains`, or `label_contains`"
        );
    }
    if matches!(args.nth, Some(0)) {
        bail!("browser element match field `nth` must be greater than or equal to 1");
    }
    Ok(())
}

fn element_matches(element: &BrowserSnapshotElement, args: &BrowserElementMatchArguments) -> bool {
    string_equals_if_present(&element.role, args.role.as_deref())
        && string_equals_if_present(&element.tag_name, args.tag_name.as_deref())
        && string_contains_if_present(&element.label, args.label_contains.as_deref())
        && string_contains_if_present(&element.text, args.text_contains.as_deref())
        && optional_string_contains_if_present(&element.href, args.href_contains.as_deref())
        && optional_string_contains_if_present(
            &element.placeholder,
            args.placeholder_contains.as_deref(),
        )
        && optional_string_equals_if_present(&element.input_type, args.input_type.as_deref())
        && optional_string_equals_if_present(&element.section_hint, args.section_hint.as_deref())
        && bool_equals_if_present(element.disabled, args.disabled)
}

fn string_equals_if_present(actual: &str, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => actual.eq_ignore_ascii_case(expected),
        None => true,
    }
}

fn optional_string_equals_if_present(actual: &Option<String>, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => actual
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case(expected))
            .unwrap_or(false),
        None => true,
    }
}

fn string_contains_if_present(actual: &str, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => contains_ignore_ascii_case(actual, expected),
        None => true,
    }
}

fn optional_string_contains_if_present(actual: &Option<String>, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => actual
            .as_deref()
            .map(|value| contains_ignore_ascii_case(value, expected))
            .unwrap_or(false),
        None => true,
    }
}

fn bool_equals_if_present(actual: bool, expected: Option<bool>) -> bool {
    match expected {
        Some(expected) => actual == expected,
        None => true,
    }
}

fn contains_ignore_ascii_case(actual: &str, expected: &str) -> bool {
    actual
        .to_ascii_lowercase()
        .contains(&expected.to_ascii_lowercase())
}

fn describe_matcher(args: &BrowserElementMatchArguments) -> String {
    let mut parts = Vec::new();
    if let Some(role) = &args.role {
        parts.push(format!("role={role}"));
    }
    if let Some(tag_name) = &args.tag_name {
        parts.push(format!("tag_name={tag_name}"));
    }
    if let Some(label_contains) = &args.label_contains {
        parts.push(format!("label_contains={label_contains}"));
    }
    if let Some(text_contains) = &args.text_contains {
        parts.push(format!("text_contains={text_contains}"));
    }
    if let Some(href_contains) = &args.href_contains {
        parts.push(format!("href_contains={href_contains}"));
    }
    if let Some(placeholder_contains) = &args.placeholder_contains {
        parts.push(format!("placeholder_contains={placeholder_contains}"));
    }
    if let Some(input_type) = &args.input_type {
        parts.push(format!("input_type={input_type}"));
    }
    if let Some(section_hint) = &args.section_hint {
        parts.push(format!("section_hint={section_hint}"));
    }
    if let Some(disabled) = args.disabled {
        parts.push(format!("disabled={disabled}"));
    }
    parts.push(format!("nth={}", args.nth.unwrap_or(1)));
    parts.join(", ")
}

fn render_match_metadata(args: &BrowserElementMatchArguments) -> serde_json::Value {
    json!({
        "role": args.role,
        "tag_name": args.tag_name,
        "label_contains": args.label_contains,
        "text_contains": args.text_contains,
        "href_contains": args.href_contains,
        "placeholder_contains": args.placeholder_contains,
        "input_type": args.input_type,
        "section_hint": args.section_hint,
        "disabled": args.disabled,
        "nth": args.nth.unwrap_or(1),
        "max_elements": resolve_match_limit(args),
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum BrowserScriptStep {
    Navigate {
        url: String,
    },
    Console {
        limit: Option<usize>,
    },
    Errors {
        limit: Option<usize>,
    },
    Requests {
        limit: Option<usize>,
        failed_only: Option<bool>,
    },
    Snapshot {
        max_elements: Option<usize>,
    },
    ClickRef {
        #[serde(rename = "ref")]
        reference: String,
    },
    ClickMatch {
        #[serde(flatten)]
        matcher: BrowserElementMatchArguments,
    },
    TypeRef {
        #[serde(rename = "ref")]
        reference: String,
        text: String,
        submit: Option<bool>,
    },
    TypeMatch {
        #[serde(flatten)]
        matcher: BrowserElementMatchArguments,
        text: String,
        submit: Option<bool>,
    },
    Screenshot {
        path: Option<PathBuf>,
    },
    Close,
}

async fn run_script_flow(
    manager: &BrowserSessionManager,
    steps_file: &Path,
    default_open_request: BrowserOpenRequest,
) -> Result<()> {
    let steps_raw = fs::read_to_string(steps_file).with_context(|| {
        format!(
            "failed to read browser script steps file {}",
            steps_file.display()
        )
    })?;
    let steps: Vec<BrowserScriptStep> = serde_json::from_str(&steps_raw).with_context(|| {
        format!(
            "failed to parse browser script steps file {}",
            steps_file.display()
        )
    })?;
    let thread_id = "internal-browser-script";
    let mut explicitly_closed = false;

    for (index, step) in steps.iter().enumerate() {
        match step {
            BrowserScriptStep::Navigate { url } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let result = manager.navigate(thread_id, url).await?;
                println!("step {} navigate: {}", index + 1, result.url);
                println!("title: {}", result.title);
            }
            BrowserScriptStep::Console { limit } => {
                let result = manager
                    .console(thread_id, BrowserDiagnosticsQuery::new(*limit))
                    .await?;
                println!("step {} console:", index + 1);
                println!("{}", render_console_entries(&result.entries));
            }
            BrowserScriptStep::Errors { limit } => {
                let result = manager
                    .errors(thread_id, BrowserDiagnosticsQuery::new(*limit))
                    .await?;
                println!("step {} errors:", index + 1);
                println!("{}", render_error_entries(&result.entries));
            }
            BrowserScriptStep::Requests { limit, failed_only } => {
                let query =
                    BrowserRequestDiagnosticsQuery::new(*limit, failed_only.unwrap_or(false));
                let result = manager.requests(thread_id, query.clone()).await?;
                println!("step {} requests:", index + 1);
                println!(
                    "{}",
                    render_request_entries(&result.entries, query.failed_only)
                );
            }
            BrowserScriptStep::Snapshot { max_elements } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let result = manager.snapshot(thread_id, *max_elements).await?;
                println!("step {} snapshot:", index + 1);
                println!("{}", result.snapshot_text);
                println!(
                    "elements: {} / {} (truncated: {})",
                    result.elements.len(),
                    result.total_candidate_count,
                    result.truncated
                );
            }
            BrowserScriptStep::ClickRef { reference } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let result = manager.click_ref(thread_id, reference).await?;
                println!("step {} click_ref: {}", index + 1, reference);
                println!("url: {}", result.url);
                println!("title: {}", result.title);
                println!("opened_new_page: {}", result.opened_new_page);
            }
            BrowserScriptStep::ClickMatch { matcher } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let matched = resolve_element_match(manager, thread_id, matcher).await?;
                let result = manager.click_ref(thread_id, &matched.reference).await?;
                println!(
                    "step {} click_match: {}",
                    index + 1,
                    describe_matcher(matcher)
                );
                println!(
                    "matched_ref: {} ({}/{})",
                    matched.reference, matched.role, matched.tag_name
                );
                if let Some(href) = matched.href.as_deref() {
                    println!("matched_href: {href}");
                }
                println!("url: {}", result.url);
                println!("title: {}", result.title);
                println!("opened_new_page: {}", result.opened_new_page);
            }
            BrowserScriptStep::TypeRef {
                reference,
                text,
                submit,
            } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let result = manager
                    .type_ref(thread_id, reference, text, submit.unwrap_or(false))
                    .await?;
                println!("step {} type_ref: {}", index + 1, reference);
                println!("url: {}", result.url);
                println!("title: {}", result.title);
                println!("submitted: {}", result.submitted);
                println!("opened_new_page: {}", result.opened_new_page);
            }
            BrowserScriptStep::TypeMatch {
                matcher,
                text,
                submit,
            } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let matched = resolve_element_match(manager, thread_id, matcher).await?;
                let result = manager
                    .type_ref(thread_id, &matched.reference, text, submit.unwrap_or(false))
                    .await?;
                println!(
                    "step {} type_match: {}",
                    index + 1,
                    describe_matcher(matcher)
                );
                println!(
                    "matched_ref: {} ({}/{})",
                    matched.reference, matched.role, matched.tag_name
                );
                println!("url: {}", result.url);
                println!("title: {}", result.title);
                println!("submitted: {}", result.submitted);
                println!("opened_new_page: {}", result.opened_new_page);
            }
            BrowserScriptStep::Screenshot { path } => {
                if let Some(open) =
                    ensure_helper_session(manager, thread_id, &default_open_request).await?
                {
                    println!(
                        "step {} open: mode={:?} url={} cookies_loaded={}",
                        index + 1,
                        open.mode,
                        open.url,
                        open.cookies_loaded
                    );
                }
                let result = manager.screenshot(thread_id, path.as_deref()).await?;
                println!("step {} screenshot: {}", index + 1, result.path);
            }
            BrowserScriptStep::Close => {
                let result = manager.close(thread_id).await?;
                explicitly_closed = true;
                println!(
                    "step {} close: had_session={}",
                    index + 1,
                    result.had_session
                );
                if let Some(artifacts) = result.artifacts {
                    println!("artifacts: {}", artifacts.session_dir.display());
                }
            }
        }
    }

    if !explicitly_closed {
        let result = manager.close(thread_id).await?;
        if let Some(artifacts) = result.artifacts {
            println!("artifacts: {}", artifacts.session_dir.display());
        }
    }

    Ok(())
}

async fn run_mock_sidecar() -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut state = MockBrowserState::from_env();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: BrowserSidecarRequest = serde_json::from_str(&line)
            .with_context(|| format!("failed to parse mock browser sidecar request: {line}"))?;
        let response = match request.payload {
            BrowserSidecarRequestPayload::Open(open_request) => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Open(state.open(open_request)),
            ),
            BrowserSidecarRequestPayload::Navigate { url } => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Navigate(state.navigate(url)),
            ),
            BrowserSidecarRequestPayload::Console(query) => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Console(state.console(query)),
            ),
            BrowserSidecarRequestPayload::Errors(query) => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Errors(state.errors(query)),
            ),
            BrowserSidecarRequestPayload::Requests(query) => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Requests(state.requests(query)),
            ),
            BrowserSidecarRequestPayload::Snapshot { max_elements } => {
                BrowserSidecarResponse::success(
                    request.id,
                    BrowserSidecarResponsePayload::Snapshot(state.snapshot(max_elements)),
                )
            }
            BrowserSidecarRequestPayload::ClickRef { reference } => {
                match state.click_ref(&reference) {
                    Ok(result) => BrowserSidecarResponse::success(
                        request.id,
                        BrowserSidecarResponsePayload::ClickRef(result),
                    ),
                    Err(error) => BrowserSidecarResponse::failure(
                        request.id,
                        "missing_ref",
                        error.to_string(),
                    ),
                }
            }
            BrowserSidecarRequestPayload::TypeRef {
                reference,
                text,
                submit,
            } => match state.type_ref(&reference, &text, submit) {
                Ok(result) => BrowserSidecarResponse::success(
                    request.id,
                    BrowserSidecarResponsePayload::TypeRef(result),
                ),
                Err(error) => {
                    BrowserSidecarResponse::failure(request.id, "missing_ref", error.to_string())
                }
            },
            BrowserSidecarRequestPayload::Screenshot { path } => match state.screenshot(&path) {
                Ok(result) => BrowserSidecarResponse::success(
                    request.id,
                    BrowserSidecarResponsePayload::Screenshot(result),
                ),
                Err(error) => BrowserSidecarResponse::failure(
                    request.id,
                    "screenshot_failed",
                    error.to_string(),
                ),
            },
            BrowserSidecarRequestPayload::ExportCookies { path } => {
                match state.export_cookies(&path) {
                    Ok(result) => BrowserSidecarResponse::success(
                        request.id,
                        BrowserSidecarResponsePayload::ExportCookies(result),
                    ),
                    Err(error) => BrowserSidecarResponse::failure(
                        request.id,
                        "export_cookies_failed",
                        error.to_string(),
                    ),
                }
            }
            BrowserSidecarRequestPayload::Close => {
                let response = match state.close() {
                    Ok(result) => BrowserSidecarResponse::success(
                        request.id,
                        BrowserSidecarResponsePayload::Close(result),
                    ),
                    Err(error) => BrowserSidecarResponse::failure(
                        request.id,
                        "close_failed",
                        error.to_string(),
                    ),
                };
                let encoded = serde_json::to_string(&response)?;
                writer.write_all(encoded.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                break;
            }
        };

        let encoded = serde_json::to_string(&response)?;
        writer.write_all(encoded.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

struct MockBrowserState {
    current_url: String,
    title: String,
    elements: Vec<BrowserSnapshotElement>,
    session_mode: Option<BrowserSessionMode>,
    session_dir: Option<PathBuf>,
    keep_artifacts: bool,
    cookies_state_file: Option<PathBuf>,
    load_cookies_on_open: bool,
    save_cookies_on_close: bool,
    mock_cookie_count: usize,
    console_entries: Vec<BrowserConsoleEntry>,
    error_entries: Vec<BrowserErrorEntry>,
    request_entries: Vec<BrowserRequestEntry>,
}

impl MockBrowserState {
    fn from_env() -> Self {
        Self {
            current_url: String::new(),
            title: String::new(),
            elements: Vec::new(),
            session_mode: None,
            session_dir: env::var_os("OPENJARVIS_BROWSER_SESSION_DIR").map(PathBuf::from),
            keep_artifacts: read_mock_bool_env("OPENJARVIS_BROWSER_KEEP_ARTIFACTS"),
            cookies_state_file: env::var_os("OPENJARVIS_BROWSER_COOKIES_STATE_FILE")
                .map(PathBuf::from),
            load_cookies_on_open: read_mock_bool_env("OPENJARVIS_BROWSER_LOAD_COOKIES_ON_OPEN"),
            save_cookies_on_close: read_mock_bool_env("OPENJARVIS_BROWSER_SAVE_COOKIES_ON_CLOSE"),
            mock_cookie_count: read_mock_usize_env("OPENJARVIS_BROWSER_MOCK_COOKIE_COUNT"),
            console_entries: Vec::new(),
            error_entries: Vec::new(),
            request_entries: Vec::new(),
        }
    }

    fn open(&mut self, request: BrowserOpenRequest) -> BrowserOpenResult {
        self.session_mode = Some(request.mode);
        if self.current_url.is_empty() {
            self.current_url = "about:blank".to_string();
        }
        if self.title.is_empty() {
            self.title = "Untitled".to_string();
        }
        let cookies_loaded =
            if matches!(request.mode, BrowserSessionMode::Launch) && self.load_cookies_on_open {
                self.load_cookie_count_from_state_file().unwrap_or(0)
            } else {
                0
            };

        BrowserOpenResult {
            mode: request.mode,
            url: self.current_url.clone(),
            title: self.title.clone(),
            cookies_loaded,
        }
    }

    fn navigate(&mut self, url: String) -> BrowserNavigateResult {
        self.current_url = url;
        self.title = mock_title(&self.current_url);
        self.record_console(BrowserConsoleEntry {
            timestamp: mock_timestamp(),
            level: BrowserConsoleLevel::Info,
            text: format!("Navigated to {}", self.current_url),
            page_url: self.current_url.clone(),
            location: None,
        });
        self.record_request(BrowserRequestEntry {
            timestamp: mock_timestamp(),
            method: "GET".to_string(),
            url: self.current_url.clone(),
            resource_type: "document".to_string(),
            status: Some(200),
            result: BrowserRequestResultKind::Ok,
        });
        if self.current_url.contains("error") {
            self.record_error(BrowserErrorEntry {
                timestamp: mock_timestamp(),
                kind: BrowserErrorKind::PageError,
                message: "mock page error".to_string(),
                page_url: Some(self.current_url.clone()),
                request_url: None,
                reason: None,
            });
        }
        if self.current_url.contains("fail") || self.current_url.contains("error") {
            let failed_request_url =
                format!("{}/api/mock-fail", self.current_url.trim_end_matches('/'));
            self.record_request(BrowserRequestEntry {
                timestamp: mock_timestamp(),
                method: "GET".to_string(),
                url: failed_request_url.clone(),
                resource_type: "xhr".to_string(),
                status: None,
                result: BrowserRequestResultKind::Failed,
            });
            self.record_error(BrowserErrorEntry {
                timestamp: mock_timestamp(),
                kind: BrowserErrorKind::RequestFailed,
                message: "mock request failed".to_string(),
                page_url: Some(self.current_url.clone()),
                request_url: Some(failed_request_url),
                reason: Some("net::ERR_FAILED".to_string()),
            });
        }
        BrowserNavigateResult {
            url: self.current_url.clone(),
            title: self.title.clone(),
        }
    }

    fn console(&self, query: BrowserDiagnosticsQuery) -> BrowserConsoleResult {
        BrowserConsoleResult {
            entries: select_mock_entries(&self.console_entries, query.limit),
        }
    }

    fn errors(&self, query: BrowserDiagnosticsQuery) -> BrowserErrorsResult {
        BrowserErrorsResult {
            entries: select_mock_entries(&self.error_entries, query.limit),
        }
    }

    fn requests(&self, query: BrowserRequestDiagnosticsQuery) -> BrowserRequestsResult {
        let filtered = self
            .request_entries
            .iter()
            .filter(|entry| {
                !query.failed_only
                    || matches!(
                        entry.result,
                        BrowserRequestResultKind::HttpError | BrowserRequestResultKind::Failed
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        BrowserRequestsResult {
            entries: select_mock_entries(&filtered, query.limit),
        }
    }

    fn snapshot(&mut self, max_elements: Option<usize>) -> BrowserSnapshotResult {
        self.elements = vec![
            BrowserSnapshotElement {
                reference: "1".to_string(),
                tag_name: "a".to_string(),
                role: "link".to_string(),
                label: "More information".to_string(),
                text: "More information".to_string(),
                selector: "body > a:nth-of-type(1)".to_string(),
                href: Some("https://example.com/more".to_string()),
                target: None,
                input_type: None,
                placeholder: None,
                section_hint: Some("main".to_string()),
                disabled: false,
            },
            BrowserSnapshotElement {
                reference: "2".to_string(),
                tag_name: "input".to_string(),
                role: "textbox".to_string(),
                label: "Search".to_string(),
                text: String::new(),
                selector: "body > input:nth-of-type(1)".to_string(),
                href: None,
                target: None,
                input_type: Some("search".to_string()),
                placeholder: Some("Search".to_string()),
                section_hint: Some("main".to_string()),
                disabled: false,
            },
        ];
        let limit = max_elements.unwrap_or(self.elements.len());
        let visible_elements = self
            .elements
            .iter()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let snapshot_text = format!(
            "URL: {}\nTitle: {}\n[1] link More information -> https://example.com/more\n[2] textbox Search",
            self.current_url_or_blank(),
            self.title_or_blank()
        );
        BrowserSnapshotResult {
            url: self.current_url_or_blank(),
            title: self.title_or_blank(),
            snapshot_text,
            elements: visible_elements,
            total_candidate_count: self.elements.len(),
            truncated: limit < self.elements.len(),
        }
    }

    fn click_ref(&self, reference: &str) -> Result<BrowserActionResult> {
        self.ensure_ref(reference)?;
        Ok(BrowserActionResult {
            reference: reference.to_string(),
            url: self.current_url_or_blank(),
            title: self.title_or_blank(),
            opened_new_page: false,
        })
    }

    fn type_ref(&self, reference: &str, text: &str, submit: bool) -> Result<BrowserTypeResult> {
        self.ensure_ref(reference)?;
        Ok(BrowserTypeResult {
            reference: reference.to_string(),
            url: self.current_url_or_blank(),
            title: self.title_or_blank(),
            text_length: text.chars().count(),
            submitted: submit,
            opened_new_page: false,
        })
    }

    fn screenshot(&self, path: &str) -> Result<BrowserScreenshotResult> {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"mock-browser-screenshot")?;
        Ok(BrowserScreenshotResult {
            url: self.current_url_or_blank(),
            title: self.title_or_blank(),
            path: path.display().to_string(),
        })
    }

    fn export_cookies(&self, path: &str) -> Result<super::BrowserCookiesExportResult> {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let cookies = self.mock_cookies();
        fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "cookies": cookies,
            })
            .to_string(),
        )?;
        Ok(super::BrowserCookiesExportResult {
            mode: self.session_mode.unwrap_or(BrowserSessionMode::Launch),
            path: path.display().to_string(),
            cookie_count: self.mock_cookie_count,
        })
    }

    fn close(&mut self) -> Result<BrowserCloseResult> {
        let mode = self.session_mode.take();
        self.ensure_diagnostic_artifact_files()?;
        let (exported_cookies_path, exported_cookie_count) =
            if matches!(mode, Some(BrowserSessionMode::Launch)) && self.save_cookies_on_close {
                if let Some(path) = self.cookies_state_file.clone() {
                    let export = self.export_cookies(path.to_string_lossy().as_ref())?;
                    (Some(export.path), Some(export.cookie_count))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
        Ok(BrowserCloseResult {
            closed: mode.is_some(),
            mode,
            exported_cookies_path,
            exported_cookie_count,
        })
    }

    fn record_console(&mut self, entry: BrowserConsoleEntry) {
        self.console_entries.push(entry.clone());
        trim_mock_buffer(&mut self.console_entries);
        let _ = self.append_diagnostic_record("console.jsonl", &entry);
    }

    fn record_error(&mut self, entry: BrowserErrorEntry) {
        self.error_entries.push(entry.clone());
        trim_mock_buffer(&mut self.error_entries);
        let _ = self.append_diagnostic_record("errors.jsonl", &entry);
    }

    fn record_request(&mut self, entry: BrowserRequestEntry) {
        self.request_entries.push(entry.clone());
        trim_mock_buffer(&mut self.request_entries);
        let _ = self.append_diagnostic_record("requests.jsonl", &entry);
    }

    fn ensure_diagnostic_artifact_files(&self) -> Result<()> {
        if !self.keep_artifacts {
            return Ok(());
        }
        let Some(session_dir) = &self.session_dir else {
            return Ok(());
        };
        fs::create_dir_all(session_dir)?;
        for file_name in ["console.jsonl", "errors.jsonl", "requests.jsonl"] {
            let path = session_dir.join(file_name);
            if !path.exists() {
                fs::write(path, b"")?;
            }
        }
        Ok(())
    }

    fn append_diagnostic_record<T>(&self, file_name: &str, entry: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        if !self.keep_artifacts {
            return Ok(());
        }
        self.ensure_diagnostic_artifact_files()?;
        let Some(session_dir) = &self.session_dir else {
            return Ok(());
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session_dir.join(file_name))?;
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }

    fn ensure_ref(&self, reference: &str) -> Result<()> {
        if self
            .elements
            .iter()
            .any(|element| element.reference == reference)
        {
            Ok(())
        } else {
            bail!("unknown browser ref `{reference}`")
        }
    }

    fn current_url_or_blank(&self) -> String {
        if self.current_url.is_empty() {
            "about:blank".to_string()
        } else {
            self.current_url.clone()
        }
    }

    fn title_or_blank(&self) -> String {
        if self.title.is_empty() {
            "Blank".to_string()
        } else {
            self.title.clone()
        }
    }

    fn load_cookie_count_from_state_file(&self) -> Result<usize> {
        let Some(path) = &self.cookies_state_file else {
            return Ok(0);
        };
        if !path.exists() {
            return Ok(0);
        }
        let raw = fs::read_to_string(path)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;
        Ok(parsed
            .get("cookies")
            .and_then(|cookies| cookies.as_array())
            .map(|cookies| cookies.len())
            .unwrap_or(0))
    }

    fn mock_cookies(&self) -> Vec<serde_json::Value> {
        (0..self.mock_cookie_count)
            .map(|index| {
                serde_json::json!({
                    "name": format!("mock-cookie-{index}"),
                    "value": format!("value-{index}"),
                    "domain": "example.com",
                    "path": "/",
                    "expires": -1,
                    "httpOnly": false,
                    "secure": true,
                    "sameSite": "Lax",
                })
            })
            .collect()
    }
}

fn select_mock_entries<T>(entries: &[T], limit: Option<usize>) -> Vec<T>
where
    T: Clone,
{
    let resolved_limit = limit.unwrap_or(20).clamp(1, 200);
    entries.iter().rev().take(resolved_limit).cloned().collect()
}

fn trim_mock_buffer<T>(entries: &mut Vec<T>) {
    const MOCK_BUFFER_LIMIT: usize = 200;
    if entries.len() > MOCK_BUFFER_LIMIT {
        let overflow = entries.len() - MOCK_BUFFER_LIMIT;
        entries.drain(0..overflow);
    }
}

fn mock_timestamp() -> String {
    Utc::now().to_rfc3339()
}

fn read_mock_bool_env(name: &str) -> bool {
    env::var(name)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn read_mock_usize_env(name: &str) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn mock_title(url: &str) -> String {
    if url.contains("example.com") {
        "Example Domain".to_string()
    } else if url.contains("localhost") {
        "Localhost".to_string()
    } else {
        "Mock Browser Page".to_string()
    }
}
