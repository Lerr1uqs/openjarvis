//! Browser automation toolset powered by a Node Playwright sidecar.

pub mod protocol;
pub mod service;
pub mod session;
pub mod tool;

use std::path::PathBuf;

pub use protocol::{
    BrowserActionResult, BrowserCloseResult, BrowserConsoleEntry, BrowserConsoleLevel,
    BrowserConsoleLocation, BrowserConsoleResult, BrowserCookiesExportResult,
    BrowserDiagnosticsQuery, BrowserErrorEntry, BrowserErrorKind, BrowserErrorsResult,
    BrowserNavigateResult, BrowserOpenRequest, BrowserOpenResult, BrowserRequestDiagnosticsQuery,
    BrowserRequestEntry, BrowserRequestResultKind, BrowserRequestsResult, BrowserScreenshotResult,
    BrowserSessionMode, BrowserSidecarError, BrowserSidecarRequest, BrowserSidecarRequestPayload,
    BrowserSidecarResponse, BrowserSidecarResponsePayload, BrowserSnapshotElement,
    BrowserSnapshotResult, BrowserTypeResult,
};
pub use service::{
    BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSidecarService,
    BrowserSidecarServiceConfig,
};
pub use session::{
    BrowserSessionArtifacts, BrowserSessionCloseOutcome, BrowserSessionManager,
    BrowserSessionManagerConfig,
};
pub use tool::{
    BrowserToolsetRuntime, register_browser_toolset, register_browser_toolset_with_config,
    run_internal_browser_command,
};

/// Return the default absolute path of the Node browser sidecar script bundled with the repo.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::tool::browser::default_sidecar_script_path;
///
/// assert!(default_sidecar_script_path().ends_with("scripts/browser_sidecar.mjs"));
/// ```
pub fn default_sidecar_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("browser_sidecar.mjs")
}
