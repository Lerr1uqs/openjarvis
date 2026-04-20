//! Runtime config, vault skeleton helpers, and CLI-backed note operations for the `obswiki`
//! toolset.

use crate::config::{AgentObswikiToolConfig, DEFAULT_OBSWIKI_VAULT_RELATIVE_PATH};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};
use tokio::process::Command as TokioCommand;
use tracing::{debug, info, warn};

pub const OBSWIKI_RAW_DIR_NAME: &str = "raw";
pub const OBSWIKI_WIKI_DIR_NAME: &str = "wiki";
pub const OBSWIKI_SCHEMA_DIR_NAME: &str = "schema";
pub const OBSWIKI_INDEX_FILE_NAME: &str = "index.md";
pub const OBSWIKI_AGENTS_FILE_NAME: &str = "AGENTS.md";
pub const OBSWIKI_SCHEMA_README_RELATIVE_PATH: &str = "schema/README.md";

/// One normalized `obswiki` runtime config derived from [`AgentObswikiToolConfig`].
///
/// # 示例
/// ```rust
/// use openjarvis::{agent::tool::obswiki::ObswikiRuntimeConfig, config::AppConfig};
///
/// let config = AppConfig::from_yaml_str(
///     r#"
/// agent:
///   tool:
///     obswiki:
///       enabled: true
///       vault_path: "/tmp/obswiki"
/// llm:
///   protocol: "mock"
///   provider: "mock"
/// "#,
/// )
/// .expect("config should parse");
///
/// let runtime = ObswikiRuntimeConfig::from_agent_config(
///     config.agent_config().tool_config().obswiki_config(),
/// )
/// .expect("obswiki runtime should resolve");
///
/// assert_eq!(runtime.vault_path().to_string_lossy(), "/tmp/obswiki");
/// assert_eq!(runtime.obsidian_bin(), "obsidian");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObswikiRuntimeConfig {
    vault_path: PathBuf,
    obsidian_bin: String,
    qmd_bin: Option<String>,
}

impl ObswikiRuntimeConfig {
    /// Resolve one enabled runtime config snapshot from YAML-backed tool config.
    pub fn from_agent_config(config: &AgentObswikiToolConfig) -> Option<Self> {
        if !config.enabled() {
            return None;
        }

        Some(Self {
            vault_path: config.vault_path().to_path_buf(),
            obsidian_bin: config.obsidian_bin().to_string(),
            qmd_bin: config.qmd_bin().map(ToOwned::to_owned),
        })
    }

    /// Return the configured vault root path.
    pub fn vault_path(&self) -> &Path {
        &self.vault_path
    }

    /// Return the executable used to invoke the Obsidian CLI.
    pub fn obsidian_bin(&self) -> &str {
        &self.obsidian_bin
    }

    /// Return the optional QMD CLI executable.
    pub fn qmd_bin(&self) -> Option<&str> {
        self.qmd_bin.as_deref()
    }

    /// Return whether this config points at the workspace-managed default vault.
    pub fn uses_default_workspace_vault(&self, workspace_root: &Path) -> bool {
        self.vault_path == workspace_root.join(DEFAULT_OBSWIKI_VAULT_RELATIVE_PATH)
    }
}

/// One resolved vault skeleton layout under one Obsidian vault root.
///
/// # 示例
/// ```rust
/// use openjarvis::agent::tool::obswiki::{
///     ObswikiVaultLayout, OBSWIKI_AGENTS_FILE_NAME, OBSWIKI_INDEX_FILE_NAME,
/// };
///
/// let layout = ObswikiVaultLayout::new("/tmp/obswiki");
/// assert!(layout.agents_file().ends_with(OBSWIKI_AGENTS_FILE_NAME));
/// assert!(layout.index_file().ends_with(OBSWIKI_INDEX_FILE_NAME));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObswikiVaultLayout {
    root: PathBuf,
}

impl ObswikiVaultLayout {
    /// Build one vault layout from the provided root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Return the default workspace-local vault layout under `./.openjarvis/obswiki/`.
    pub fn default_for_workspace(workspace_root: impl AsRef<Path>) -> Self {
        Self::new(
            workspace_root
                .as_ref()
                .join(DEFAULT_OBSWIKI_VAULT_RELATIVE_PATH),
        )
    }

    /// Return the vault root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the `raw/` directory path.
    pub fn raw_dir(&self) -> PathBuf {
        self.root.join(OBSWIKI_RAW_DIR_NAME)
    }

    /// Return the `wiki/` directory path.
    pub fn wiki_dir(&self) -> PathBuf {
        self.root.join(OBSWIKI_WIKI_DIR_NAME)
    }

    /// Return the `schema/` directory path.
    pub fn schema_dir(&self) -> PathBuf {
        self.root.join(OBSWIKI_SCHEMA_DIR_NAME)
    }

    /// Return the vault root `index.md` path.
    pub fn index_file(&self) -> PathBuf {
        self.root.join(OBSWIKI_INDEX_FILE_NAME)
    }

    /// Return the vault root `AGENTS.md` path.
    pub fn agents_file(&self) -> PathBuf {
        self.root.join(OBSWIKI_AGENTS_FILE_NAME)
    }

    /// Return the default schema readme path.
    pub fn schema_readme_file(&self) -> PathBuf {
        self.root.join(OBSWIKI_SCHEMA_README_RELATIVE_PATH)
    }

    /// Create the default workspace-managed vault skeleton when it does not exist yet.
    ///
    /// Returns `true` when at least one directory or file was newly created.
    pub fn ensure_default_skeleton(&self) -> Result<bool> {
        let mut changed = false;
        if !self.root.exists() {
            fs::create_dir_all(&self.root).with_context(|| {
                format!("failed to create obswiki root {}", self.root.display())
            })?;
            changed = true;
        }

        for directory in [self.raw_dir(), self.wiki_dir(), self.schema_dir()] {
            if directory.exists() {
                continue;
            }
            fs::create_dir_all(&directory).with_context(|| {
                format!("failed to create obswiki directory {}", directory.display())
            })?;
            changed = true;
        }

        changed |= write_default_file_if_missing(
            &self.agents_file(),
            default_agents_md(self.root()),
            "obswiki AGENTS.md",
        )?;
        changed |= write_default_file_if_missing(
            &self.index_file(),
            default_index_md(),
            "obswiki index.md",
        )?;
        changed |= write_default_file_if_missing(
            &self.schema_readme_file(),
            default_schema_readme_md(),
            "obswiki schema README",
        )?;
        if changed {
            info!(vault_root = %self.root.display(), "ensured default obswiki vault skeleton");
        }
        Ok(changed)
    }
}

/// Structured preflight snapshot used by thread init and debug output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiPreflightStatus {
    pub vault_path: PathBuf,
    pub obsidian_cli_available: bool,
    pub qmd_configured: bool,
    pub qmd_cli_available: bool,
    pub raw_dir_exists: bool,
    pub wiki_dir_exists: bool,
    pub schema_dir_exists: bool,
    pub index_file_exists: bool,
    pub agents_file_exists: bool,
}

impl ObswikiPreflightStatus {
    /// Return whether the required vault skeleton exists.
    pub fn skeleton_complete(&self) -> bool {
        self.raw_dir_exists
            && self.wiki_dir_exists
            && self.schema_dir_exists
            && self.index_file_exists
            && self.agents_file_exists
    }
}

/// One parsed Obsidian note with optional YAML frontmatter metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiDocumentMetadata {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

/// One fully materialized `obswiki` note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiDocument {
    pub path: String,
    pub metadata: ObswikiDocumentMetadata,
    pub content: String,
}

/// One structured search candidate returned by `obswiki_search`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiSearchCandidate {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub backend: String,
}

/// Structured search response shared by the runtime and tool contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiSearchResponse {
    pub query: String,
    pub backend: String,
    pub total_matches: usize,
    pub items: Vec<ObswikiSearchCandidate>,
}

/// Stable vault snapshot injected into the `obswiki` child-thread system prefix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObswikiVaultContext {
    pub preflight: ObswikiPreflightStatus,
    pub agents_body: String,
    pub index_body: String,
}

/// Supported deterministic update instructions used by `obswiki_update`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObswikiUpdateInstruction {
    ReplaceAll {
        content: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        page_type: Option<String>,
    },
    ReplaceOne {
        old_text: String,
        new_text: String,
    },
    Append {
        content: String,
    },
    Prepend {
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObswikiFrontmatter {
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    source_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_path: Option<String>,
}

/// CLI-backed vault runtime used by the `obswiki` toolset and child-thread init.
#[derive(Debug)]
pub struct ObswikiRuntime {
    config: ObswikiRuntimeConfig,
    layout: ObswikiVaultLayout,
}

impl ObswikiRuntime {
    /// Create one runtime wrapper from an enabled config snapshot.
    pub fn new(config: ObswikiRuntimeConfig) -> Self {
        let layout = ObswikiVaultLayout::new(config.vault_path().to_path_buf());
        Self { config, layout }
    }

    /// Return the normalized runtime config.
    pub fn config(&self) -> &ObswikiRuntimeConfig {
        &self.config
    }

    /// Return the resolved vault layout.
    pub fn layout(&self) -> &ObswikiVaultLayout {
        &self.layout
    }

    /// Ensure the workspace-managed default vault exists before stricter preflight runs.
    pub fn ensure_default_workspace_vault(&self, workspace_root: &Path) -> Result<bool> {
        if !self.config.uses_default_workspace_vault(workspace_root) {
            return Ok(false);
        }
        self.layout.ensure_default_skeleton()
    }

    /// Run the full vault and CLI preflight check.
    ///
    /// This fails fast when the vault root is missing, required files are absent, or the
    /// Obsidian CLI cannot be invoked. QMD remains optional and only updates the returned status.
    pub fn preflight(&self) -> Result<ObswikiPreflightStatus> {
        debug!(
            vault_path = %self.config.vault_path().display(),
            obsidian_bin = %self.config.obsidian_bin(),
            qmd_configured = self.config.qmd_bin().is_some(),
            "starting obswiki preflight"
        );
        if !self.layout.root().exists() {
            bail!(
                "obswiki vault root does not exist: {}",
                self.layout.root().display()
            );
        }
        if !self.layout.root().is_dir() {
            bail!(
                "obswiki vault root is not a directory: {}",
                self.layout.root().display()
            );
        }

        let raw_dir_exists = self.layout.raw_dir().is_dir();
        let wiki_dir_exists = self.layout.wiki_dir().is_dir();
        let schema_dir_exists = self.layout.schema_dir().is_dir();
        let index_file_exists = self.layout.index_file().is_file();
        let agents_file_exists = self.layout.agents_file().is_file();
        let qmd_configured = self.config.qmd_bin().is_some();
        let qmd_cli_available = self
            .config
            .qmd_bin()
            .map(|bin| probe_cli(bin, &["--help"], self.layout.root()))
            .transpose()?
            .unwrap_or(false);
        let status = ObswikiPreflightStatus {
            vault_path: self.layout.root().to_path_buf(),
            obsidian_cli_available: false,
            qmd_configured,
            qmd_cli_available,
            raw_dir_exists,
            wiki_dir_exists,
            schema_dir_exists,
            index_file_exists,
            agents_file_exists,
        };

        if !status.skeleton_complete() {
            let missing = missing_skeleton_entries(&status);
            bail!(
                "obswiki vault skeleton is incomplete under {}: missing {}",
                self.layout.root().display(),
                missing.join(", ")
            );
        }
        let obsidian_cli_available =
            probe_cli(self.config.obsidian_bin(), &["--help"], self.layout.root())?;
        if !obsidian_cli_available {
            bail!(
                "obswiki obsidian cli `{}` is not available for vault {}",
                self.config.obsidian_bin(),
                self.layout.root().display()
            );
        }
        let expected_agents_body =
            fs::read_to_string(self.layout.agents_file()).with_context(|| {
                format!(
                    "failed to read obswiki AGENTS.md {} during runtime probe",
                    self.layout.agents_file().display()
                )
            })?;
        if !probe_obsidian_runtime(
            self.config.obsidian_bin(),
            self.layout.root(),
            &expected_agents_body,
        )? {
            bail!(
                "obswiki obsidian cli `{}` could not reach a running Obsidian app bound to vault {}; open that vault in Obsidian manually and retry",
                self.config.obsidian_bin(),
                self.layout.root().display()
            );
        }
        let status = ObswikiPreflightStatus {
            obsidian_cli_available,
            ..status
        };
        info!(
            vault_path = %status.vault_path.display(),
            qmd_configured = status.qmd_configured,
            qmd_cli_available = status.qmd_cli_available,
            "completed obswiki preflight"
        );
        Ok(status)
    }

    /// Load the stable vault context injected into the `obswiki` child thread.
    pub async fn load_vault_context(&self) -> Result<ObswikiVaultContext> {
        let preflight = self.preflight()?;
        let agents_body = self.read_raw_markdown(OBSWIKI_AGENTS_FILE_NAME).await?;
        let index_body = self.read_raw_markdown(OBSWIKI_INDEX_FILE_NAME).await?;
        Ok(ObswikiVaultContext {
            preflight,
            agents_body,
            index_body,
        })
    }

    /// Read one markdown document by vault-relative path.
    pub async fn read_document(&self, path: &str) -> Result<ObswikiDocument> {
        let normalized = validate_obswiki_markdown_path(path)?;
        let raw = self
            .read_raw_markdown(&pathbuf_to_slash_string(&normalized))
            .await?;
        parse_obswiki_document(&pathbuf_to_slash_string(&normalized), &raw)
    }

    /// Search structured candidates with QMD lexical matching first and Obsidian fallback.
    pub async fn search(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<ObswikiSearchResponse> {
        let query = query.trim();
        if query.is_empty() {
            bail!("obswiki search query must not be blank");
        }
        let limit = limit.max(1);
        let normalized_scope = normalize_obswiki_scope(scope)?;

        if self.preflight()?.qmd_cli_available {
            match self
                .search_with_qmd(query, normalized_scope.as_deref(), limit)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => warn!(
                    query,
                    scope = normalized_scope.as_deref().unwrap_or("all"),
                    error = %error,
                    "obswiki qmd search failed, falling back to obsidian"
                ),
            }
        }

        self.search_with_obsidian(query, normalized_scope.as_deref(), limit)
            .await
    }

    /// Import one markdown file into the immutable `raw/` layer.
    pub async fn import_raw_markdown(
        &self,
        source_path: &Path,
        title: Option<&str>,
        source_uri: Option<&str>,
    ) -> Result<ObswikiDocument> {
        let source_path = source_path.canonicalize().with_context(|| {
            format!(
                "failed to resolve source markdown {}",
                source_path.display()
            )
        })?;
        if source_path.extension().and_then(|value| value.to_str()) != Some("md") {
            bail!("obswiki_import_raw only accepts markdown `.md` files");
        }
        let raw_content = fs::read_to_string(&source_path)
            .with_context(|| format!("failed to read source markdown {}", source_path.display()))?;
        let title = normalize_title(title.map(str::to_string).unwrap_or_else(|| {
            derive_title_from_raw(&source_path_to_relative_title(&source_path), &raw_content)
        }))?;
        let now = Utc::now();
        let metadata = ObswikiDocumentMetadata {
            title: title.clone(),
            page_type: Some("raw_import".to_string()),
            created_at: Some(now),
            updated_at: Some(now),
            links: Vec::new(),
            source_refs: Vec::new(),
            source_uri: normalize_optional_string(source_uri),
            source_path: Some(source_path.display().to_string()),
        };
        let destination = self.next_available_raw_path(&title).await?;
        let rendered = render_obswiki_markdown(&metadata, &raw_content)?;
        self.write_markdown(&pathbuf_to_slash_string(&destination), &rendered, false)
            .await?;
        self.refresh_index().await?;
        info!(
            source_path = %source_path.display(),
            imported_path = %destination.display(),
            "imported markdown into obswiki raw layer"
        );
        self.read_document(&pathbuf_to_slash_string(&destination))
            .await
    }

    /// Create or overwrite one mutable `wiki/` or `schema/` page and refresh the root index.
    pub async fn write_document(
        &self,
        path: &str,
        title: &str,
        content: &str,
        page_type: Option<&str>,
        links: Option<&[String]>,
        source_refs: Option<&[String]>,
    ) -> Result<ObswikiDocument> {
        let normalized = validate_obswiki_markdown_path(path)?;
        if !is_mutable_obswiki_path(&normalized) {
            bail!("obswiki write only allows paths under `wiki/` or `schema/`");
        }

        let existing = self.try_existing_document(&normalized).await?;
        let now = Utc::now();
        let metadata = ObswikiDocumentMetadata {
            title: normalize_title(title.to_string())?,
            page_type: normalize_optional_string(page_type),
            created_at: existing
                .as_ref()
                .and_then(|document| document.metadata.created_at)
                .or(Some(now)),
            updated_at: Some(now),
            links: normalize_string_list(links.unwrap_or(&[]))?,
            source_refs: normalize_string_list(source_refs.unwrap_or(&[]))?,
            source_uri: existing
                .as_ref()
                .and_then(|document| document.metadata.source_uri.clone()),
            source_path: existing
                .as_ref()
                .and_then(|document| document.metadata.source_path.clone()),
        };
        let rendered = render_obswiki_markdown(&metadata, content)?;
        self.write_markdown(&pathbuf_to_slash_string(&normalized), &rendered, true)
            .await?;
        self.refresh_index().await?;
        info!(
            path = %normalized.display(),
            title = %metadata.title,
            link_count = metadata.links.len(),
            "wrote obswiki managed document"
        );
        self.read_document(&pathbuf_to_slash_string(&normalized))
            .await
    }

    /// Apply one deterministic update instruction to an existing mutable page.
    pub async fn update_document(
        &self,
        path: &str,
        instruction: ObswikiUpdateInstruction,
        expected_links: Option<&[String]>,
        source_refs: Option<&[String]>,
    ) -> Result<ObswikiDocument> {
        let normalized = validate_obswiki_markdown_path(path)?;
        if is_raw_obswiki_path(&normalized) {
            bail!("obswiki update must not modify the immutable `raw/` layer");
        }
        if !is_mutable_obswiki_path(&normalized) {
            bail!("obswiki update only allows paths under `wiki/` or `schema/`");
        }

        let mut document = self
            .read_document(&pathbuf_to_slash_string(&normalized))
            .await?;
        apply_update_instruction(&mut document, instruction)?;
        if let Some(expected_links) = expected_links {
            document.metadata.links = normalize_string_list(expected_links)?;
        }
        if let Some(source_refs) = source_refs {
            document.metadata.source_refs = normalize_string_list(source_refs)?;
        }
        document.metadata.updated_at = Some(Utc::now());
        let rendered = render_obswiki_markdown(&document.metadata, &document.content)?;
        self.write_markdown(&pathbuf_to_slash_string(&normalized), &rendered, true)
            .await?;
        self.refresh_index().await?;
        info!(
            path = %normalized.display(),
            title = %document.metadata.title,
            "updated obswiki managed document"
        );
        self.read_document(&pathbuf_to_slash_string(&normalized))
            .await
    }

    async fn search_with_qmd(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<ObswikiSearchResponse> {
        let Some(qmd_bin) = self.config.qmd_bin() else {
            bail!("qmd cli is not configured");
        };
        let args = vec![
            "search".to_string(),
            "--json".to_string(),
            "-n".to_string(),
            limit.to_string(),
            query.to_string(),
        ];
        let stdout = run_async_command(qmd_bin, &args, self.layout.root()).await?;
        let json: Value =
            serde_json::from_str(&stdout).context("failed to parse qmd search json output")?;
        let mut candidates = Vec::new();
        collect_qmd_candidates(&json, self.layout.root(), &mut candidates);
        let candidates = filter_candidates_by_scope(candidates, scope);
        Ok(ObswikiSearchResponse {
            query: query.to_string(),
            backend: "qmd".to_string(),
            total_matches: candidates.len(),
            items: candidates.into_iter().take(limit).collect(),
        })
    }

    async fn search_with_obsidian(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<ObswikiSearchResponse> {
        let mut args = vec![
            "search".to_string(),
            format!("query={query}"),
            format!("limit={limit}"),
            "format=json".to_string(),
        ];
        if let Some(scope) = scope {
            args.push(format!("path={scope}"));
        }
        let stdout = self.run_obsidian_cli_command(&args).await?;
        let paths = parse_obsidian_path_list(&stdout, self.layout.root())
            .context("failed to parse obsidian search results")?;
        let items = self
            .enrich_path_candidates("obsidian", paths, limit)
            .await?;
        Ok(ObswikiSearchResponse {
            query: query.to_string(),
            backend: "obsidian".to_string(),
            total_matches: items.len(),
            items,
        })
    }

    async fn enrich_path_candidates(
        &self,
        backend: &str,
        paths: Vec<String>,
        limit: usize,
    ) -> Result<Vec<ObswikiSearchCandidate>> {
        let mut candidates = Vec::new();
        for path in paths.into_iter().take(limit) {
            let document = self.read_document(&path).await?;
            candidates.push(ObswikiSearchCandidate {
                path,
                title: Some(document.metadata.title),
                summary: Some(build_summary(&document.content)),
                backend: backend.to_string(),
            });
        }
        Ok(candidates)
    }

    async fn read_raw_markdown(&self, path: &str) -> Result<String> {
        let args = vec!["read".to_string(), format!("path={path}")];
        let stdout = self.run_obsidian_cli_command(&args).await?;
        Ok(stdout)
    }

    async fn write_markdown(&self, path: &str, content: &str, overwrite: bool) -> Result<()> {
        let mut args = vec![
            "create".to_string(),
            format!("path={path}"),
            format!("content={content}"),
        ];
        if overwrite {
            args.push("overwrite".to_string());
        }
        let _ = self.run_obsidian_cli_command(&args).await?;
        Ok(())
    }

    async fn list_markdown_paths(&self, folder: &str) -> Result<Vec<String>> {
        let args = vec![
            "files".to_string(),
            format!("folder={folder}"),
            "ext=md".to_string(),
        ];
        let stdout = self.run_obsidian_cli_command(&args).await?;
        parse_obsidian_path_list(&stdout, self.layout.root())
    }

    async fn refresh_index(&self) -> Result<()> {
        let mut all_paths = Vec::new();
        for folder in [
            OBSWIKI_RAW_DIR_NAME,
            OBSWIKI_WIKI_DIR_NAME,
            OBSWIKI_SCHEMA_DIR_NAME,
        ] {
            all_paths.extend(self.list_markdown_paths(folder).await?);
        }
        all_paths.sort();
        all_paths.dedup();

        let mut lines = vec![
            "# Obswiki Index".to_string(),
            String::new(),
            "系统会在每次导入 Raw 或写回 wiki/schema 页面后自动刷新这里。".to_string(),
            String::new(),
        ];
        if all_paths.is_empty() {
            lines.push("当前暂无条目。".to_string());
        } else {
            for path in all_paths {
                let document = self.read_document(&path).await?;
                let summary = build_summary(&document.content);
                lines.push(format!("- [{}|{}]", path, summary));
            }
        }
        let index = format!("{}\n", lines.join("\n"));
        self.write_markdown(OBSWIKI_INDEX_FILE_NAME, &index, true)
            .await?;
        info!(vault_path = %self.layout.root().display(), "refreshed obswiki root index");
        Ok(())
    }

    async fn next_available_raw_path(&self, title: &str) -> Result<PathBuf> {
        let stem = slugify_note_stem(title);
        let mut attempt = 1usize;
        loop {
            let file_name = if attempt == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem}-{attempt}.md")
            };
            let candidate = PathBuf::from(OBSWIKI_RAW_DIR_NAME).join(file_name);
            if !self.layout.root().join(&candidate).exists() {
                return Ok(candidate);
            }
            attempt += 1;
        }
    }

    async fn try_existing_document(&self, path: &Path) -> Result<Option<ObswikiDocument>> {
        let absolute = self.layout.root().join(path);
        if !absolute.exists() {
            return Ok(None);
        }
        self.read_document(&pathbuf_to_slash_string(path))
            .await
            .map(Some)
    }

    async fn run_obsidian_cli_command(&self, args: &[String]) -> Result<String> {
        run_async_command(self.config.obsidian_bin(), args, self.layout.root()).await
    }
}

fn write_default_file_if_missing(path: &Path, content: String, label: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        bail!("{label} path is missing a parent directory");
    };
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    fs::write(path, content)
        .with_context(|| format!("failed to write {label} {}", path.display()))?;
    Ok(true)
}

fn default_agents_md(root: &Path) -> String {
    format!(
        concat!(
            "# Obswiki Vault Instructions\n\n",
            "当前 vault 根目录: `{}`\n\n",
            "## 目录职责\n\n",
            "- `raw/`: 只存放导入后的 markdown 原文，agent 后续不得改写。\n",
            "- `wiki/`: 只存放 LLM 整理后的知识页。\n",
            "- `schema/`: 只存放模板、约束、页面规范与校验说明。\n",
            "- `index.md`: 系统自动维护的 `[链接|摘要]` 根索引。\n\n",
            "## 工作约束\n\n",
            "- 所有受管页面读取、搜索、写入与更新都必须走 `obswiki` 工具，不要绕过 vault 约束直接改文件。\n",
            "- 需要先检索候选，再显式读取正文，不要只根据搜索结果直接下结论。\n",
            "- 写回只允许落到 `wiki/` 或 `schema/`，严禁改写 `raw/`。\n",
            "- 每次写回后系统会自动刷新 `index.md`，不要手动维护索引。\n"
        ),
        root.display()
    )
}

fn default_index_md() -> String {
    concat!(
        "# Obswiki Index\n\n",
        "系统会在每次导入 Raw 或写回 wiki/schema 页面后自动刷新这里。\n\n",
        "当前暂无条目。\n"
    )
    .to_string()
}

fn default_schema_readme_md() -> String {
    concat!(
        "# Schema Notes\n\n",
        "这里存放页面模板、字段约束、更新规则和校验说明。\n\n",
        "- 将通用页面模板写在这里。\n",
        "- 将页面更新规范写在这里。\n",
        "- 不要把实际知识页放到 `schema/`。\n"
    )
    .to_string()
}

fn missing_skeleton_entries(status: &ObswikiPreflightStatus) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !status.raw_dir_exists {
        missing.push(OBSWIKI_RAW_DIR_NAME);
    }
    if !status.wiki_dir_exists {
        missing.push(OBSWIKI_WIKI_DIR_NAME);
    }
    if !status.schema_dir_exists {
        missing.push(OBSWIKI_SCHEMA_DIR_NAME);
    }
    if !status.index_file_exists {
        missing.push(OBSWIKI_INDEX_FILE_NAME);
    }
    if !status.agents_file_exists {
        missing.push(OBSWIKI_AGENTS_FILE_NAME);
    }
    missing
}

fn probe_cli(executable: &str, args: &[&str], cwd: &Path) -> Result<bool> {
    let status = Command::new(executable)
        .current_dir(cwd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to spawn executable `{executable}`"))?;
    Ok(status.success())
}

fn probe_obsidian_runtime(
    executable: &str,
    cwd: &Path,
    expected_agents_body: &str,
) -> Result<bool> {
    let args = [
        "read".to_string(),
        format!("path={OBSWIKI_AGENTS_FILE_NAME}"),
    ];
    debug!(
        executable,
        cwd = %cwd.display(),
        "probing obswiki obsidian runtime via AGENTS.md read"
    );
    let output = Command::new(executable)
        .current_dir(cwd)
        .args(&args)
        .output()
        .with_context(|| format!("failed to spawn executable `{executable}`"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let actual = stdout.trim_end_matches(['\r', '\n']);
        let expected = expected_agents_body.trim_end_matches(['\r', '\n']);
        if actual == expected {
            return Ok(true);
        }
        warn!(
            executable,
            cwd = %cwd.display(),
            expected_agents_len = expected.len(),
            actual_agents_len = actual.len(),
            actual_agents_preview = %truncate_for_log(actual, 120),
            "obswiki obsidian runtime probe returned unexpected AGENTS.md content"
        );
        return Ok(false);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    warn!(
        executable,
        cwd = %cwd.display(),
        status = ?output.status.code(),
        stdout = %stdout.trim(),
        stderr = %stderr.trim(),
        "obswiki obsidian runtime probe failed"
    );
    Ok(false)
}

fn truncate_for_log(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    value.chars().take(max_len).collect::<String>() + "..."
}

async fn run_async_command(executable: &str, args: &[String], cwd: &Path) -> Result<String> {
    run_async_command_once(executable, args, cwd).await
}

async fn run_async_command_once(executable: &str, args: &[String], cwd: &Path) -> Result<String> {
    debug!(
        executable,
        cwd = %cwd.display(),
        arg_count = args.len(),
        "starting obswiki cli command"
    );
    let output = TokioCommand::new(executable)
        .current_dir(cwd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("failed to spawn executable `{executable}`"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        bail!(
            "obswiki cli `{executable}` failed with status {:?}: stdout=`{}` stderr=`{}`",
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(stdout)
}

/// Validate one vault-relative markdown path used by managed note APIs.
pub fn validate_obswiki_markdown_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("obswiki path must not be blank");
    }

    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        bail!("obswiki path must be relative to the vault root");
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("obswiki path must not contain parent directory traversal");
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        bail!("obswiki path must stay inside the vault root");
    }
    if candidate.extension().and_then(|value| value.to_str()) != Some("md") {
        bail!("obswiki path must end with `.md`");
    }

    let normalized = candidate
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<OsString>>();
    if normalized.is_empty() {
        bail!("obswiki path must contain at least one normal component");
    }

    Ok(normalized.into_iter().collect())
}

/// Return whether the target path belongs to one mutable managed layer.
pub fn is_mutable_obswiki_path(path: &Path) -> bool {
    let Some(first_component) = path.components().next() else {
        return false;
    };
    matches!(
        first_component,
        Component::Normal(value)
            if value == OBSWIKI_WIKI_DIR_NAME || value == OBSWIKI_SCHEMA_DIR_NAME
    )
}

/// Return whether the target path points at the immutable `raw/` layer.
pub fn is_raw_obswiki_path(path: &Path) -> bool {
    let Some(first_component) = path.components().next() else {
        return false;
    };
    matches!(first_component, Component::Normal(value) if value == OBSWIKI_RAW_DIR_NAME)
}

/// Parse one deterministic `obswiki_update` instruction string.
///
/// The parser first accepts YAML or JSON objects tagged with `operation`. When parsing fails, the
/// whole string falls back to one `replace_all` payload so callers can still provide full markdown
/// content directly.
pub fn parse_obswiki_update_instruction(instructions: &str) -> Result<ObswikiUpdateInstruction> {
    let trimmed = instructions.trim();
    if trimmed.is_empty() {
        bail!("obswiki update instructions must not be blank");
    }
    match serde_yaml::from_str::<ObswikiUpdateInstruction>(trimmed) {
        Ok(instruction) => Ok(instruction),
        Err(_) => Ok(ObswikiUpdateInstruction::ReplaceAll {
            content: instructions.to_string(),
            title: None,
            page_type: None,
        }),
    }
}

fn parse_obsidian_path_list(stdout: &str, vault_root: &Path) -> Result<Vec<String>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
        let mut paths = Vec::new();
        collect_path_like_strings(&json, vault_root, &mut paths);
        paths.sort();
        paths.dedup();
        return Ok(paths);
    }

    let mut paths = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| normalize_cli_path(line, vault_root))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn collect_qmd_candidates(
    value: &Value,
    vault_root: &Path,
    candidates: &mut Vec<ObswikiSearchCandidate>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_qmd_candidates(item, vault_root, candidates);
            }
        }
        Value::Object(object) => {
            let path = object
                .get("path")
                .or_else(|| object.get("file"))
                .or_else(|| object.get("note"))
                .and_then(Value::as_str)
                .and_then(|value| normalize_cli_path(value, vault_root));
            if let Some(path) = path {
                let title = object
                    .get("title")
                    .or_else(|| object.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let summary = object
                    .get("summary")
                    .or_else(|| object.get("snippet"))
                    .or_else(|| object.get("excerpt"))
                    .or_else(|| object.get("content"))
                    .and_then(Value::as_str)
                    .map(collapse_whitespace);
                candidates.push(ObswikiSearchCandidate {
                    path,
                    title,
                    summary,
                    backend: "qmd".to_string(),
                });
            }
            for key in ["items", "results", "matches", "hits", "data"] {
                if let Some(child) = object.get(key) {
                    collect_qmd_candidates(child, vault_root, candidates);
                }
            }
        }
        _ => {}
    }
}

fn collect_path_like_strings(value: &Value, vault_root: &Path, paths: &mut Vec<String>) {
    match value {
        Value::String(raw) => {
            if let Some(path) = normalize_cli_path(raw, vault_root) {
                paths.push(path);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_path_like_strings(item, vault_root, paths);
            }
        }
        Value::Object(object) => {
            for key in ["path", "file", "note", "note_path", "relative_path"] {
                if let Some(raw) = object.get(key).and_then(Value::as_str)
                    && let Some(path) = normalize_cli_path(raw, vault_root)
                {
                    paths.push(path);
                }
            }
            for key in [
                "items", "results", "matches", "hits", "files", "notes", "data",
            ] {
                if let Some(child) = object.get(key) {
                    collect_path_like_strings(child, vault_root, paths);
                }
            }
        }
        _ => {}
    }
}

fn normalize_cli_path(raw: &str, vault_root: &Path) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || !raw.ends_with(".md") {
        return None;
    }

    let candidate = PathBuf::from(raw);
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(vault_root).ok()?.to_path_buf()
    } else {
        candidate
    };
    let normalized = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<OsString>>();
    if normalized.is_empty() {
        return None;
    }
    Some(pathbuf_to_slash_string(
        &normalized.into_iter().collect::<PathBuf>(),
    ))
}

fn parse_obswiki_document(path: &str, raw: &str) -> Result<ObswikiDocument> {
    if let Some((frontmatter, body)) = split_frontmatter(raw) {
        let parsed: ObswikiFrontmatter = serde_yaml::from_str(frontmatter)
            .with_context(|| format!("failed to parse obswiki frontmatter for `{path}`"))?;
        let title = normalize_title(parsed.title)?;
        return Ok(ObswikiDocument {
            path: path.to_string(),
            metadata: ObswikiDocumentMetadata {
                title,
                page_type: normalize_optional_owned(parsed.page_type),
                created_at: parsed.created_at,
                updated_at: parsed.updated_at,
                links: normalize_string_list(&parsed.links)?,
                source_refs: normalize_string_list(&parsed.source_refs)?,
                source_uri: normalize_optional_owned(parsed.source_uri),
                source_path: normalize_optional_owned(parsed.source_path),
            },
            content: body.trim_end_matches('\n').to_string(),
        });
    }

    Ok(ObswikiDocument {
        path: path.to_string(),
        metadata: ObswikiDocumentMetadata {
            title: derive_title_from_raw(path, raw),
            page_type: None,
            created_at: None,
            updated_at: None,
            links: Vec::new(),
            source_refs: Vec::new(),
            source_uri: None,
            source_path: None,
        },
        content: raw.trim_end_matches('\n').to_string(),
    })
}

fn render_obswiki_markdown(metadata: &ObswikiDocumentMetadata, content: &str) -> Result<String> {
    let frontmatter = ObswikiFrontmatter {
        title: metadata.title.clone(),
        page_type: metadata.page_type.clone(),
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        links: metadata.links.clone(),
        source_refs: metadata.source_refs.clone(),
        source_uri: metadata.source_uri.clone(),
        source_path: metadata.source_path.clone(),
    };
    let yaml =
        serde_yaml::to_string(&frontmatter).context("failed to render obswiki frontmatter")?;
    let yaml = yaml.trim();
    let body = content.trim_end_matches('\n');
    Ok(format!("---\n{yaml}\n---\n{body}\n"))
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let stripped = raw.strip_prefix("---\n")?;
    let index = stripped.find("\n---\n")?;
    let frontmatter = &stripped[..index];
    let body = &stripped[(index + "\n---\n".len())..];
    Some((frontmatter, body))
}

fn normalize_title(title: String) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        bail!("obswiki title must not be blank");
    }
    Ok(title.to_string())
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_optional_owned(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_string_list(values: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::<String>::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("obswiki string list must not contain blank values");
        }
        if !normalized.iter().any(|existing| existing == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_obswiki_scope(scope: Option<&str>) -> Result<Option<String>> {
    let Some(scope) = scope.map(str::trim).filter(|scope| !scope.is_empty()) else {
        return Ok(None);
    };
    if scope == "all" {
        return Ok(None);
    }
    let normalized = scope.replace('\\', "/");
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        bail!("obswiki search scope must be relative to the vault root");
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("obswiki search scope must not contain parent directory traversal");
    }
    Ok(Some(
        candidate
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => value.to_str().map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

fn filter_candidates_by_scope(
    candidates: Vec<ObswikiSearchCandidate>,
    scope: Option<&str>,
) -> Vec<ObswikiSearchCandidate> {
    let Some(scope) = scope else {
        return candidates;
    };
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.path == scope || candidate.path.starts_with(&format!("{scope}/"))
        })
        .collect()
}

fn derive_title_from_raw(path: &str, raw: &str) -> String {
    raw.lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("obswiki-note")
                .to_string()
        })
}

fn build_summary(content: &str) -> String {
    let summary = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(collapse_whitespace)
        .unwrap_or_else(|| "无摘要".to_string());
    truncate_chars(&summary, 120)
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let truncated = value.chars().take(max_chars).collect::<String>();
    format!("{truncated}...")
}

fn slugify_note_stem(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

fn source_path_to_relative_title(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("source.md")
        .to_string()
}

fn pathbuf_to_slash_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn apply_update_instruction(
    document: &mut ObswikiDocument,
    instruction: ObswikiUpdateInstruction,
) -> Result<()> {
    match instruction {
        ObswikiUpdateInstruction::ReplaceAll {
            content,
            title,
            page_type,
        } => {
            document.content = content;
            if let Some(title) = title {
                document.metadata.title = normalize_title(title)?;
            }
            if let Some(page_type) = page_type {
                document.metadata.page_type = normalize_optional_owned(Some(page_type));
            }
        }
        ObswikiUpdateInstruction::ReplaceOne { old_text, new_text } => {
            if old_text.is_empty() {
                bail!("obswiki replace_one update requires non-empty `old_text`");
            }
            if !document.content.contains(&old_text) {
                bail!(
                    "obswiki update did not find target text in `{}`",
                    document.path
                );
            }
            document.content = document.content.replacen(&old_text, &new_text, 1);
        }
        ObswikiUpdateInstruction::Append { content } => {
            let fragment = content.trim_end_matches('\n');
            if !document.content.is_empty() && !document.content.ends_with('\n') {
                document.content.push('\n');
            }
            document.content.push_str(fragment);
        }
        ObswikiUpdateInstruction::Prepend { content } => {
            let fragment = content.trim_end_matches('\n');
            document.content = if document.content.is_empty() {
                fragment.to_string()
            } else {
                format!("{fragment}\n{}", document.content)
            };
        }
    }
    Ok(())
}
