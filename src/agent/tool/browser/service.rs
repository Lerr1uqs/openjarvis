//! Sidecar process management and JSON-line transport for browser automation.

use super::protocol::{
    BrowserActionResult, BrowserCloseResult, BrowserCookiesExportResult, BrowserNavigateResult,
    BrowserOpenRequest, BrowserOpenResult, BrowserScreenshotResult, BrowserSessionMode,
    BrowserSidecarRequest, BrowserSidecarRequestPayload, BrowserSidecarResponse,
    BrowserSidecarResponsePayload, BrowserSnapshotResult, BrowserTypeResult,
};
use anyhow::{Context, Result, bail};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use tracing::{debug, info};

/// Executable command line used to launch one browser sidecar process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserProcessCommandSpec {
    pub executable: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

impl BrowserProcessCommandSpec {
    /// Build the default Node sidecar command for the provided script path.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserProcessCommandSpec;
    ///
    /// let spec = BrowserProcessCommandSpec::node_sidecar("scripts/browser_sidecar.mjs");
    /// assert_eq!(spec.executable, "node");
    /// ```
    pub fn node_sidecar(script_path: impl Into<PathBuf>) -> Self {
        Self {
            executable: "node".to_string(),
            args: vec![script_path.into().display().to_string()],
            env: HashMap::new(),
        }
    }

    /// Append one environment variable override to the process spec.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserProcessCommandSpec;
    ///
    /// let spec = BrowserProcessCommandSpec::node_sidecar("scripts/browser_sidecar.mjs")
    ///     .with_env("OPENJARVIS_BROWSER_HEADLESS", "1");
    /// assert_eq!(spec.env["OPENJARVIS_BROWSER_HEADLESS"], "1");
    /// ```
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// Runtime options passed into each browser sidecar process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserRuntimeOptions {
    pub headless: bool,
    pub keep_artifacts: bool,
    pub chrome_executable: Option<PathBuf>,
    pub cookies_state_file: Option<PathBuf>,
    pub load_cookies_on_open: bool,
    pub save_cookies_on_close: bool,
    pub launch_timeout_ms: u64,
}

impl Default for BrowserRuntimeOptions {
    fn default() -> Self {
        Self {
            headless: true,
            keep_artifacts: false,
            chrome_executable: None,
            cookies_state_file: None,
            load_cookies_on_open: false,
            save_cookies_on_close: false,
            launch_timeout_ms: 30_000,
        }
    }
}

/// Full process configuration for one browser sidecar session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSidecarServiceConfig {
    pub process: BrowserProcessCommandSpec,
    pub runtime: BrowserRuntimeOptions,
    pub session_root: PathBuf,
    pub user_data_dir: PathBuf,
}

impl BrowserSidecarServiceConfig {
    /// Create a new service config for one isolated browser session.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::{
    ///     BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSidecarServiceConfig,
    /// };
    /// use std::path::PathBuf;
    ///
    /// let config = BrowserSidecarServiceConfig::new(
    ///     BrowserProcessCommandSpec::node_sidecar("scripts/browser_sidecar.mjs"),
    ///     BrowserRuntimeOptions::default(),
    ///     PathBuf::from("tmp/browser"),
    ///     PathBuf::from("tmp/browser/user-data"),
    /// );
    /// assert_eq!(config.process.executable, "node");
    /// ```
    pub fn new(
        process: BrowserProcessCommandSpec,
        runtime: BrowserRuntimeOptions,
        session_root: PathBuf,
        user_data_dir: PathBuf,
    ) -> Self {
        Self {
            process,
            runtime,
            session_root,
            user_data_dir,
        }
    }
}

/// Lazy browser sidecar client bound to one isolated session directory.
pub struct BrowserSidecarService {
    config: BrowserSidecarServiceConfig,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<Lines<BufReader<ChildStdout>>>,
    next_request_id: u64,
    session_opened: bool,
}

impl BrowserSidecarService {
    /// Create a new sidecar client that starts lazily on the first browser action.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::{
    ///     BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSidecarService,
    ///     BrowserSidecarServiceConfig,
    /// };
    /// use std::path::PathBuf;
    ///
    /// let service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
    ///     BrowserProcessCommandSpec::node_sidecar("scripts/browser_sidecar.mjs"),
    ///     BrowserRuntimeOptions::default(),
    ///     PathBuf::from("tmp/browser"),
    ///     PathBuf::from("tmp/browser/user-data"),
    /// ));
    /// assert!(!service.is_started());
    /// ```
    pub fn new(config: BrowserSidecarServiceConfig) -> Self {
        Self {
            config,
            child: None,
            stdin: None,
            stdout: None,
            next_request_id: 1,
            session_opened: false,
        }
    }

    /// Return whether the sidecar child process has already been started.
    pub fn is_started(&self) -> bool {
        self.child.is_some()
    }

    /// Return whether the current sidecar already owns one opened browser session.
    pub fn is_session_open(&self) -> bool {
        self.session_opened
    }

    /// Open one browser session explicitly with the provided mode and optional CDP endpoint.
    pub async fn open(&mut self, request: BrowserOpenRequest) -> Result<BrowserOpenResult> {
        validate_open_request(&request)?;
        let result = match self
            .call(BrowserSidecarRequestPayload::Open(request.clone()))
            .await?
        {
            BrowserSidecarResponsePayload::Open(result) => result,
            other => bail!(
                "browser sidecar returned `{}` for open",
                response_action_name(&other)
            ),
        };
        self.session_opened = true;
        info!(
            mode = ?result.mode,
            url = %result.url,
            title = %result.title,
            cookies_loaded = result.cookies_loaded,
            "opened browser session"
        );
        Ok(result)
    }

    /// Navigate the browser session to the provided URL.
    pub async fn navigate(&mut self, url: &str) -> Result<BrowserNavigateResult> {
        self.ensure_opened_with_default_launch().await?;
        match self
            .call(BrowserSidecarRequestPayload::Navigate {
                url: url.to_string(),
            })
            .await?
        {
            BrowserSidecarResponsePayload::Navigate(result) => Ok(result),
            other => bail!(
                "browser sidecar returned `{}` for navigate",
                response_action_name(&other)
            ),
        }
    }

    /// Capture the current browser snapshot and element refs.
    pub async fn snapshot(&mut self, max_elements: Option<usize>) -> Result<BrowserSnapshotResult> {
        self.ensure_opened_with_default_launch().await?;
        match self
            .call(BrowserSidecarRequestPayload::Snapshot { max_elements })
            .await?
        {
            BrowserSidecarResponsePayload::Snapshot(result) => Ok(result),
            other => bail!(
                "browser sidecar returned `{}` for snapshot",
                response_action_name(&other)
            ),
        }
    }

    /// Click one element ref produced by a prior snapshot.
    pub async fn click_ref(&mut self, reference: &str) -> Result<BrowserActionResult> {
        self.ensure_opened_with_default_launch().await?;
        match self
            .call(BrowserSidecarRequestPayload::ClickRef {
                reference: reference.to_string(),
            })
            .await?
        {
            BrowserSidecarResponsePayload::ClickRef(result) => Ok(result),
            other => bail!(
                "browser sidecar returned `{}` for click_ref",
                response_action_name(&other)
            ),
        }
    }

    /// Type text into one element ref produced by a prior snapshot.
    pub async fn type_ref(
        &mut self,
        reference: &str,
        text: &str,
        submit: bool,
    ) -> Result<BrowserTypeResult> {
        self.ensure_opened_with_default_launch().await?;
        match self
            .call(BrowserSidecarRequestPayload::TypeRef {
                reference: reference.to_string(),
                text: text.to_string(),
                submit,
            })
            .await?
        {
            BrowserSidecarResponsePayload::TypeRef(result) => Ok(result),
            other => bail!(
                "browser sidecar returned `{}` for type_ref",
                response_action_name(&other)
            ),
        }
    }

    /// Write a screenshot to the provided absolute or relative file path.
    pub async fn screenshot(&mut self, path: &Path) -> Result<BrowserScreenshotResult> {
        self.ensure_opened_with_default_launch().await?;
        match self
            .call(BrowserSidecarRequestPayload::Screenshot {
                path: path.display().to_string(),
            })
            .await?
        {
            BrowserSidecarResponsePayload::Screenshot(result) => Ok(result),
            other => bail!(
                "browser sidecar returned `{}` for screenshot",
                response_action_name(&other)
            ),
        }
    }

    /// Export cookies from the current opened browser session into the provided path.
    pub async fn export_cookies(&mut self, path: &Path) -> Result<BrowserCookiesExportResult> {
        if !self.session_opened {
            bail!("browser session is not open");
        }
        match self
            .call(BrowserSidecarRequestPayload::ExportCookies {
                path: path.display().to_string(),
            })
            .await?
        {
            BrowserSidecarResponsePayload::ExportCookies(result) => {
                info!(
                    mode = ?result.mode,
                    path = %result.path,
                    cookie_count = result.cookie_count,
                    "exported browser cookies"
                );
                Ok(result)
            }
            other => bail!(
                "browser sidecar returned `{}` for export_cookies",
                response_action_name(&other)
            ),
        }
    }

    /// Close the browser sidecar session and wait for the child process to exit.
    pub async fn close(&mut self) -> Result<BrowserCloseResult> {
        if !self.is_started() {
            return Ok(BrowserCloseResult {
                closed: false,
                mode: None,
                exported_cookies_path: None,
                exported_cookie_count: None,
            });
        }

        let response = self.call(BrowserSidecarRequestPayload::Close).await;
        let _ = self.shutdown_process().await;
        match response? {
            BrowserSidecarResponsePayload::Close(result) => {
                self.session_opened = false;
                if result.closed {
                    info!(
                        mode = ?result.mode,
                        exported_cookies_path = ?result.exported_cookies_path,
                        exported_cookie_count = ?result.exported_cookie_count,
                        "closed browser session"
                    );
                }
                Ok(result)
            }
            other => bail!(
                "browser sidecar returned `{}` for close",
                response_action_name(&other)
            ),
        }
    }

    async fn call(
        &mut self,
        payload: BrowserSidecarRequestPayload,
    ) -> Result<BrowserSidecarResponsePayload> {
        self.ensure_started().await?;

        let request_id = format!("browser-{}", self.next_request_id);
        self.next_request_id += 1;
        let request = BrowserSidecarRequest::new(request_id.clone(), payload);
        debug!(
            request_id = %request_id,
            action = %request_action_name(&request.payload),
            "sending browser sidecar request"
        );
        let encoded = serde_json::to_string(&request)
            .context("failed to serialize browser sidecar request")?;

        let stdin = self
            .stdin
            .as_mut()
            .context("browser sidecar stdin is not available")?;
        stdin
            .write_all(encoded.as_bytes())
            .await
            .context("failed to write browser sidecar request")?;
        stdin
            .write_all(b"\n")
            .await
            .context("failed to terminate browser sidecar request line")?;
        stdin
            .flush()
            .await
            .context("failed to flush browser sidecar request")?;

        let stdout = self
            .stdout
            .as_mut()
            .context("browser sidecar stdout is not available")?;
        let line = stdout
            .next_line()
            .await
            .context("failed to read browser sidecar response")?
            .context("browser sidecar exited before returning a response")?;
        let response: BrowserSidecarResponse = serde_json::from_str(&line)
            .with_context(|| format!("failed to decode browser sidecar response line: {line}"))?;
        if response.id != request_id {
            bail!(
                "browser sidecar response id mismatch: expected `{request_id}`, got `{}`",
                response.id
            );
        }

        if response.ok {
            let result = response
                .result
                .context("browser sidecar returned `ok=true` without a result payload")?;
            debug!(
                request_id = %request_id,
                action = %response_action_name(&result),
                "received browser sidecar success response"
            );
            Ok(result)
        } else {
            let error = response
                .error
                .context("browser sidecar returned `ok=false` without an error payload")?;
            debug!(
                request_id = %request_id,
                error_code = %error.code,
                error_message = %error.message,
                "browser sidecar returned error response"
            );
            bail!("browser sidecar `{}`: {}", error.code, error.message);
        }
    }

    async fn ensure_opened_with_default_launch(&mut self) -> Result<()> {
        if self.session_opened {
            return Ok(());
        }
        let _ = self.open(BrowserOpenRequest::launch()).await?;
        Ok(())
    }

    async fn ensure_started(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }

        let mut command = Command::new(&self.config.process.executable);
        command
            .args(&self.config.process.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for (key, value) in &self.config.process.env {
            command.env(key, value);
        }
        command.env(
            "OPENJARVIS_BROWSER_HEADLESS",
            if self.config.runtime.headless {
                "1"
            } else {
                "0"
            },
        );
        command.env(
            "OPENJARVIS_BROWSER_KEEP_ARTIFACTS",
            if self.config.runtime.keep_artifacts {
                "1"
            } else {
                "0"
            },
        );
        command.env(
            "OPENJARVIS_BROWSER_SESSION_DIR",
            self.config.session_root.display().to_string(),
        );
        command.env(
            "OPENJARVIS_BROWSER_USER_DATA_DIR",
            self.config.user_data_dir.display().to_string(),
        );
        command.env(
            "OPENJARVIS_BROWSER_LAUNCH_TIMEOUT_MS",
            self.config.runtime.launch_timeout_ms.to_string(),
        );
        if let Some(chrome_executable) = &self.config.runtime.chrome_executable {
            command.env(
                "OPENJARVIS_BROWSER_CHROME_PATH",
                chrome_executable.display().to_string(),
            );
        }
        if let Some(cookies_state_file) = &self.config.runtime.cookies_state_file {
            command.env(
                "OPENJARVIS_BROWSER_COOKIES_STATE_FILE",
                cookies_state_file.display().to_string(),
            );
        }
        command.env(
            "OPENJARVIS_BROWSER_LOAD_COOKIES_ON_OPEN",
            if self.config.runtime.load_cookies_on_open {
                "1"
            } else {
                "0"
            },
        );
        command.env(
            "OPENJARVIS_BROWSER_SAVE_COOKIES_ON_CLOSE",
            if self.config.runtime.save_cookies_on_close {
                "1"
            } else {
                "0"
            },
        );

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn browser sidecar executable `{}`",
                self.config.process.executable
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .context("failed to capture browser sidecar stdout")?;
        let stdin = child
            .stdin
            .take()
            .context("failed to capture browser sidecar stdin")?;

        self.child = Some(child);
        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout).lines());
        Ok(())
    }

    async fn shutdown_process(&mut self) -> Result<()> {
        self.stdin.take();
        self.stdout.take();
        if let Some(mut child) = self.child.take() {
            match timeout(Duration::from_secs(2), child.wait()).await {
                Ok(wait_result) => {
                    wait_result.context("failed to wait for browser sidecar exit")?;
                }
                Err(_) => {
                    child
                        .kill()
                        .await
                        .context("failed to kill browser sidecar after timeout")?;
                    child
                        .wait()
                        .await
                        .context("failed to wait for killed browser sidecar")?;
                }
            }
        }
        Ok(())
    }
}

fn response_action_name(payload: &BrowserSidecarResponsePayload) -> &'static str {
    match payload {
        BrowserSidecarResponsePayload::Open(_) => "open",
        BrowserSidecarResponsePayload::Navigate(_) => "navigate",
        BrowserSidecarResponsePayload::Snapshot(_) => "snapshot",
        BrowserSidecarResponsePayload::ClickRef(_) => "click_ref",
        BrowserSidecarResponsePayload::TypeRef(_) => "type_ref",
        BrowserSidecarResponsePayload::Screenshot(_) => "screenshot",
        BrowserSidecarResponsePayload::ExportCookies(_) => "export_cookies",
        BrowserSidecarResponsePayload::Close(_) => "close",
    }
}

fn request_action_name(payload: &BrowserSidecarRequestPayload) -> &'static str {
    match payload {
        BrowserSidecarRequestPayload::Open(_) => "open",
        BrowserSidecarRequestPayload::Navigate { .. } => "navigate",
        BrowserSidecarRequestPayload::Snapshot { .. } => "snapshot",
        BrowserSidecarRequestPayload::ClickRef { .. } => "click_ref",
        BrowserSidecarRequestPayload::TypeRef { .. } => "type_ref",
        BrowserSidecarRequestPayload::Screenshot { .. } => "screenshot",
        BrowserSidecarRequestPayload::ExportCookies { .. } => "export_cookies",
        BrowserSidecarRequestPayload::Close => "close",
    }
}

fn validate_open_request(request: &BrowserOpenRequest) -> Result<()> {
    match request.mode {
        BrowserSessionMode::Launch => {
            if request.cdp_endpoint.is_some() {
                bail!("browser launch mode does not accept `cdp_endpoint`");
            }
        }
        BrowserSessionMode::Attach => {
            if request
                .cdp_endpoint
                .as_deref()
                .map(|cdp_endpoint| cdp_endpoint.trim().is_empty())
                .unwrap_or(true)
            {
                bail!("browser attach mode requires a non-empty `cdp_endpoint`");
            }
        }
    }
    Ok(())
}
