//! Browser toolset registration, tool handlers, and internal helper commands.

use super::super::{parse_tool_arguments, tool_definition_from_args};
use super::{
    default_sidecar_script_path,
    protocol::{
        BrowserActionResult, BrowserCloseResult, BrowserNavigateResult, BrowserScreenshotResult,
        BrowserSidecarRequest, BrowserSidecarRequestPayload, BrowserSidecarResponse,
        BrowserSidecarResponsePayload, BrowserSnapshotElement, BrowserSnapshotResult,
        BrowserTypeResult,
    },
    service::{BrowserProcessCommandSpec, BrowserRuntimeOptions},
    session::{BrowserSessionCloseOutcome, BrowserSessionManager, BrowserSessionManagerConfig},
};
use crate::{
    agent::{
        ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        ToolRegistry, ToolsetCatalogEntry, ToolsetRuntime, empty_tool_input_schema,
    },
    cli::InternalBrowserCommand,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
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
        .register_toolset_with_runtime(
            ToolsetCatalogEntry::new(
                "browser",
                "Browser automation tools powered by a Node Playwright sidecar.",
            ),
            vec![
                Arc::new(BrowserNavigateTool::new(Arc::clone(&sessions))),
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
            headless,
            output_dir,
            node_bin,
            script_path,
            chrome_path,
        } => {
            let manager = BrowserSessionManager::new(build_helper_manager_config(
                *headless,
                output_dir.clone(),
                node_bin.clone(),
                script_path.clone(),
                chrome_path.clone(),
                "openjarvis-browser-smoke",
            ));
            run_smoke_flow(&manager, url).await
        }
        InternalBrowserCommand::Script {
            steps_file,
            headless,
            output_dir,
            node_bin,
            script_path,
            chrome_path,
        } => {
            let manager = BrowserSessionManager::new(build_helper_manager_config(
                *headless,
                output_dir.clone(),
                node_bin.clone(),
                script_path.clone(),
                chrome_path.clone(),
                "openjarvis-browser-script",
            ));
            run_script_flow(&manager, steps_file).await
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
impl ToolHandler for BrowserNavigateTool {
    fn definition(&self) -> ToolDefinition {
        tool_definition_from_args::<BrowserNavigateArguments>(
            "browser__navigate",
            "Open one URL inside the current thread-scoped browser session.",
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
        Ok(render_navigate_result(result))
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

fn render_navigate_result(result: BrowserNavigateResult) -> ToolCallResult {
    ToolCallResult {
        content: format!("Navigated to {}.", result.url),
        metadata: json!({
            "toolset": "browser",
            "url": result.url,
            "title": result.title,
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
            "Browser session closed.".to_string()
        } else {
            "No browser session was active for the current thread.".to_string()
        },
        metadata: json!({
            "toolset": "browser",
            "had_session": result.had_session,
            "kept_artifacts": result.kept_artifacts,
            "artifacts_dir": artifacts_dir,
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
            ..Default::default()
        },
        artifact_root,
    }
}

async fn run_smoke_flow(manager: &BrowserSessionManager, url: &str) -> Result<()> {
    let thread_id = "internal-browser-smoke";
    let navigate = manager.navigate(thread_id, url).await?;
    let snapshot = manager.snapshot(thread_id, Some(120)).await?;
    let screenshot = manager.screenshot(thread_id, None).await?;
    let close = manager.close(thread_id).await?;

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

async fn run_script_flow(manager: &BrowserSessionManager, steps_file: &Path) -> Result<()> {
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
                let result = manager.navigate(thread_id, url).await?;
                println!("step {} navigate: {}", index + 1, result.url);
                println!("title: {}", result.title);
            }
            BrowserScriptStep::Snapshot { max_elements } => {
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
                let result = manager.click_ref(thread_id, reference).await?;
                println!("step {} click_ref: {}", index + 1, reference);
                println!("url: {}", result.url);
                println!("title: {}", result.title);
                println!("opened_new_page: {}", result.opened_new_page);
            }
            BrowserScriptStep::ClickMatch { matcher } => {
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
    let mut state = MockBrowserState::default();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: BrowserSidecarRequest = serde_json::from_str(&line)
            .with_context(|| format!("failed to parse mock browser sidecar request: {line}"))?;
        let response = match request.payload {
            BrowserSidecarRequestPayload::Navigate { url } => BrowserSidecarResponse::success(
                request.id,
                BrowserSidecarResponsePayload::Navigate(state.navigate(url)),
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
            BrowserSidecarRequestPayload::Close => {
                let response = BrowserSidecarResponse::success(
                    request.id,
                    BrowserSidecarResponsePayload::Close(BrowserCloseResult { closed: true }),
                );
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

#[derive(Default)]
struct MockBrowserState {
    current_url: String,
    title: String,
    elements: Vec<BrowserSnapshotElement>,
}

impl MockBrowserState {
    fn navigate(&mut self, url: String) -> BrowserNavigateResult {
        self.current_url = url;
        self.title = mock_title(&self.current_url);
        BrowserNavigateResult {
            url: self.current_url.clone(),
            title: self.title.clone(),
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
