//! Thread-scoped browser session manager that owns session directories and sidecar lifecycle.

use super::{
    default_sidecar_script_path,
    protocol::{
        BrowserActionResult, BrowserCloseResult, BrowserCookiesExportResult, BrowserNavigateResult,
        BrowserOpenRequest, BrowserOpenResult, BrowserScreenshotResult, BrowserSessionMode,
        BrowserSnapshotResult, BrowserTypeResult,
    },
    service::{
        BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSidecarService,
        BrowserSidecarServiceConfig,
    },
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

/// Filesystem locations allocated for one isolated browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSessionArtifacts {
    pub session_dir: PathBuf,
    pub user_data_dir: PathBuf,
}

/// Close outcome for one browser session managed at the thread level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSessionCloseOutcome {
    pub had_session: bool,
    pub kept_artifacts: bool,
    pub artifacts: Option<BrowserSessionArtifacts>,
    pub session_mode: Option<BrowserSessionMode>,
    pub auto_exported_path: Option<String>,
    pub exported_cookie_count: Option<usize>,
}

/// Configuration shared by all thread-scoped browser sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSessionManagerConfig {
    pub process: BrowserProcessCommandSpec,
    pub runtime: BrowserRuntimeOptions,
    pub artifact_root: PathBuf,
}

impl Default for BrowserSessionManagerConfig {
    fn default() -> Self {
        Self {
            process: BrowserProcessCommandSpec::node_sidecar(default_sidecar_script_path()),
            runtime: BrowserRuntimeOptions::default(),
            artifact_root: std::env::temp_dir().join("openjarvis-browser"),
        }
    }
}

struct ManagedBrowserSession {
    artifacts: BrowserSessionArtifacts,
    service: BrowserSidecarService,
}

/// Thread-scoped browser session owner used by browser tool handlers and helpers.
pub struct BrowserSessionManager {
    config: BrowserSessionManagerConfig,
    sessions: RwLock<HashMap<String, Arc<Mutex<ManagedBrowserSession>>>>,
}

impl BrowserSessionManager {
    /// Create a new browser session manager from the provided process and artifact config.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserSessionManager;
    ///
    /// let manager = BrowserSessionManager::new(Default::default());
    /// assert!(!manager.has_session_blocking("missing-thread"));
    /// ```
    pub fn new(config: BrowserSessionManagerConfig) -> Self {
        Self {
            config,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Return whether the target thread currently owns an allocated browser session.
    pub async fn has_session(&self, thread_id: &str) -> bool {
        self.sessions.read().await.contains_key(thread_id)
    }

    /// Blocking-friendly helper used by documentation examples and simple assertions.
    pub fn has_session_blocking(&self, thread_id: &str) -> bool {
        self.sessions.blocking_read().contains_key(thread_id)
    }

    /// Explicitly open or replace the current thread-scoped browser session.
    pub async fn open(
        &self,
        thread_id: &str,
        request: BrowserOpenRequest,
    ) -> Result<BrowserOpenResult> {
        if let Some(previous) = self.sessions.write().await.remove(thread_id) {
            let _ = self.close_managed_session_arc(thread_id, previous).await?;
        }

        let mut next_session = self.create_session(thread_id)?;
        match next_session.service.open(request).await {
            Ok(result) => {
                let session = Arc::new(Mutex::new(next_session));
                self.sessions
                    .write()
                    .await
                    .insert(thread_id.to_string(), session);
                info!(
                    thread_id,
                    mode = ?result.mode,
                    url = %result.url,
                    "opened thread-scoped browser session"
                );
                Ok(result)
            }
            Err(error) => {
                let cleanup_outcome = self.close_managed_session(thread_id, next_session).await;
                if let Err(cleanup_error) = cleanup_outcome {
                    warn!(
                        thread_id,
                        error = %cleanup_error,
                        "failed to cleanup browser session after open error"
                    );
                }
                Err(error)
            }
        }
    }

    /// Navigate one thread-scoped browser session to the provided URL.
    pub async fn navigate(&self, thread_id: &str, url: &str) -> Result<BrowserNavigateResult> {
        let session = self.session_for_thread(thread_id).await?;
        let mut session = session.lock().await;
        session.service.navigate(url).await
    }

    /// Capture a browser snapshot for the target thread.
    pub async fn snapshot(
        &self,
        thread_id: &str,
        max_elements: Option<usize>,
    ) -> Result<BrowserSnapshotResult> {
        let session = self.session_for_thread(thread_id).await?;
        let mut session = session.lock().await;
        session.service.snapshot(max_elements).await
    }

    /// Click one prior snapshot ref inside the target thread.
    pub async fn click_ref(&self, thread_id: &str, reference: &str) -> Result<BrowserActionResult> {
        let session = self.session_for_thread(thread_id).await?;
        let mut session = session.lock().await;
        session.service.click_ref(reference).await
    }

    /// Type text into one prior snapshot ref inside the target thread.
    pub async fn type_ref(
        &self,
        thread_id: &str,
        reference: &str,
        text: &str,
        submit: bool,
    ) -> Result<BrowserTypeResult> {
        let session = self.session_for_thread(thread_id).await?;
        let mut session = session.lock().await;
        session.service.type_ref(reference, text, submit).await
    }

    /// Write a screenshot for the target thread, using a generated default path when omitted.
    pub async fn screenshot(
        &self,
        thread_id: &str,
        requested_path: Option<&Path>,
    ) -> Result<BrowserScreenshotResult> {
        let session = self.session_for_thread(thread_id).await?;
        let mut session = session.lock().await;
        let screenshot_path = requested_path.map(Path::to_path_buf).unwrap_or_else(|| {
            session
                .artifacts
                .session_dir
                .join(format!("screenshot-{}.png", Uuid::new_v4()))
        });
        if let Some(parent) = screenshot_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create browser screenshot directory {}",
                    parent.display()
                )
            })?;
        }
        session.service.screenshot(&screenshot_path).await
    }

    /// Export cookies from the current active browser session into one explicit file.
    pub async fn export_cookies(
        &self,
        thread_id: &str,
        requested_path: &Path,
    ) -> Result<BrowserCookiesExportResult> {
        let Some(session) = self.sessions.read().await.get(thread_id).cloned() else {
            anyhow::bail!("no active browser session for thread `{thread_id}`");
        };
        let mut session = session.lock().await;
        if let Some(parent) = requested_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create browser cookies export directory {}",
                    parent.display()
                )
            })?;
        }
        session.service.export_cookies(requested_path).await
    }

    /// Close and remove one thread-scoped browser session.
    pub async fn close(&self, thread_id: &str) -> Result<BrowserSessionCloseOutcome> {
        let session = self.sessions.write().await.remove(thread_id);
        let Some(session) = session else {
            return Ok(BrowserSessionCloseOutcome {
                had_session: false,
                kept_artifacts: self.config.runtime.keep_artifacts,
                artifacts: None,
                session_mode: None,
                auto_exported_path: None,
                exported_cookie_count: None,
            });
        };

        self.close_managed_session_arc(thread_id, session).await
    }

    async fn session_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<Arc<Mutex<ManagedBrowserSession>>> {
        if let Some(existing) = self.sessions.read().await.get(thread_id).cloned() {
            return Ok(existing);
        }

        let session = Arc::new(Mutex::new(self.create_session(thread_id)?));
        let mut sessions = self.sessions.write().await;
        Ok(sessions
            .entry(thread_id.to_string())
            .or_insert_with(|| Arc::clone(&session))
            .clone())
    }

    fn create_session(&self, thread_id: &str) -> Result<ManagedBrowserSession> {
        fs::create_dir_all(&self.config.artifact_root).with_context(|| {
            format!(
                "failed to create browser artifact root {}",
                self.config.artifact_root.display()
            )
        })?;

        let session_dir = self.config.artifact_root.join(format!(
            "session-{}-{}",
            sanitize_segment(thread_id),
            Uuid::new_v4()
        ));
        let user_data_dir = session_dir.join("user-data");
        fs::create_dir_all(&user_data_dir).with_context(|| {
            format!(
                "failed to create browser user data directory {}",
                user_data_dir.display()
            )
        })?;

        let artifacts = BrowserSessionArtifacts {
            session_dir: session_dir.clone(),
            user_data_dir: user_data_dir.clone(),
        };
        let service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
            self.config.process.clone(),
            self.config.runtime.clone(),
            session_dir,
            user_data_dir,
        ));

        Ok(ManagedBrowserSession { artifacts, service })
    }

    async fn close_managed_session(
        &self,
        thread_id: &str,
        mut session: ManagedBrowserSession,
    ) -> Result<BrowserSessionCloseOutcome> {
        let artifacts = session.artifacts.clone();
        let BrowserCloseResult {
            closed,
            mode,
            exported_cookies_path,
            exported_cookie_count,
        } = session.service.close().await?;

        let kept_artifacts = self.config.runtime.keep_artifacts;
        if !kept_artifacts && artifacts.session_dir.exists() {
            fs::remove_dir_all(&artifacts.session_dir).with_context(|| {
                format!(
                    "failed to remove browser session directory {}",
                    artifacts.session_dir.display()
                )
            })?;
        }

        info!(
            thread_id,
            closed,
            kept_artifacts,
            session_mode = ?mode,
            exported_cookies_path = ?exported_cookies_path,
            exported_cookie_count = ?exported_cookie_count,
            "closed thread-scoped browser session"
        );

        Ok(BrowserSessionCloseOutcome {
            had_session: true,
            kept_artifacts,
            artifacts: if kept_artifacts {
                Some(artifacts)
            } else {
                None
            },
            session_mode: mode,
            auto_exported_path: exported_cookies_path,
            exported_cookie_count,
        })
    }

    async fn close_managed_session_arc(
        &self,
        thread_id: &str,
        session: Arc<Mutex<ManagedBrowserSession>>,
    ) -> Result<BrowserSessionCloseOutcome> {
        let mut session = session.lock().await;
        let artifacts = session.artifacts.clone();
        let BrowserCloseResult {
            closed,
            mode,
            exported_cookies_path,
            exported_cookie_count,
        } = session.service.close().await?;
        drop(session);

        let kept_artifacts = self.config.runtime.keep_artifacts;
        if !kept_artifacts && artifacts.session_dir.exists() {
            fs::remove_dir_all(&artifacts.session_dir).with_context(|| {
                format!(
                    "failed to remove browser session directory {}",
                    artifacts.session_dir.display()
                )
            })?;
        }

        info!(
            thread_id,
            closed,
            kept_artifacts,
            session_mode = ?mode,
            exported_cookies_path = ?exported_cookies_path,
            exported_cookie_count = ?exported_cookie_count,
            "closed thread-scoped browser session"
        );

        Ok(BrowserSessionCloseOutcome {
            had_session: true,
            kept_artifacts,
            artifacts: if kept_artifacts {
                Some(artifacts)
            } else {
                None
            },
            session_mode: mode,
            auto_exported_path: exported_cookies_path,
            exported_cookie_count,
        })
    }
}

fn sanitize_segment(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "thread".to_string()
    } else {
        sanitized
    }
}
