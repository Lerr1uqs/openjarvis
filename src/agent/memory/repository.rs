//! Markdown-backed local memory repository used by thread init and the `memory` toolset.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};
use tracing::{info, warn};

const OPENJARVIS_MEMORY_DIR: &str = ".openjarvis/memory";
const ACTIVE_MEMORY_TOOLSET_HINT: &str = "以下是当前工作区可用的 Active Memory 关键词目录。这里只暴露 `keyword -> relative path` 词表，不直接注入正文。需要详情时，请先用 `load_toolset` 加载 `memory` toolset，再调用 `memory_get`、`memory_search`、`memory_list` 或 `memory_write`。";

/// Stable memory storage namespace under the workspace-local `.openjarvis/memory` tree.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::agent::memory::MemoryRepository;
///
/// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
/// assert!(repository.memory_root().ends_with(".openjarvis/memory"));
/// ```
#[derive(Debug, Clone)]
pub struct MemoryRepository {
    workspace_root: PathBuf,
}

/// Distinguish active memory from passive memory.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    Active,
    Passive,
}

impl MemoryType {
    /// Return the stable directory name used by the repository layout.
    pub fn as_dir_name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
        }
    }

    fn iter() -> [Self; 2] {
        [Self::Active, Self::Passive]
    }
}

/// Frontmatter-backed metadata stored for one markdown memory document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDocumentMetadata {
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

/// One fully materialized memory document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDocument {
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub path: String,
    pub metadata: MemoryDocumentMetadata,
    pub content: String,
}

/// One structured search/list candidate without the full body content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDocumentSummary {
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub path: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

/// One active-memory keyword catalog entry injected during thread initialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveMemoryCatalogEntry {
    pub keyword: String,
    pub path: String,
}

/// Structured response returned by lexical search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchResponse {
    pub query: String,
    pub total_matches: usize,
    pub items: Vec<MemoryDocumentSummary>,
}

/// One validated write request handled by the repository or `memory_write` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWriteRequest {
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub path: String,
    pub title: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MemoryFrontmatter {
    title: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keywords: Vec<String>,
}

impl MemoryRepository {
    /// Create one repository bound to the target workspace root.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    /// Return the resolved `.openjarvis/memory` root directory.
    pub fn memory_root(&self) -> PathBuf {
        self.workspace_root.join(OPENJARVIS_MEMORY_DIR)
    }

    /// Return the local storage root for one memory type.
    pub fn type_root(&self, memory_type: MemoryType) -> PathBuf {
        self.memory_root().join(memory_type.as_dir_name())
    }

    /// Load one memory document by `type + path`.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::{MemoryRepository, MemoryType};
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _document = repository.get(MemoryType::Passive, "notes/example.md");
    /// ```
    pub fn get(&self, memory_type: MemoryType, path: &str) -> Result<MemoryDocument> {
        let normalized_path = validate_memory_file_path(path)?;
        let absolute_path = self.type_root(memory_type).join(&normalized_path);
        let raw = fs::read_to_string(&absolute_path)
            .with_context(|| format!("failed to read memory file {}", absolute_path.display()))?;
        parse_memory_document(memory_type, &normalized_path, &raw)
    }

    /// Write one active/passive markdown memory document to disk.
    ///
    /// `active` documents must carry non-empty `keywords`, while `passive` documents must not.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::{MemoryRepository, MemoryType, MemoryWriteRequest};
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _document = repository.write(MemoryWriteRequest {
    ///     memory_type: MemoryType::Passive,
    ///     path: "notes/user.md".to_string(),
    ///     title: "用户偏好".to_string(),
    ///     content: "用户偏好中文回答".to_string(),
    ///     keywords: None,
    /// });
    /// ```
    pub fn write(&self, request: MemoryWriteRequest) -> Result<MemoryDocument> {
        let normalized_path = validate_memory_file_path(&request.path)?;
        let title = request.title.trim();
        if title.is_empty() {
            bail!("memory title must not be blank");
        }

        let normalized_keywords = normalize_keywords(request.keywords.unwrap_or_default())?;
        match request.memory_type {
            MemoryType::Active => {
                if normalized_keywords.is_empty() {
                    bail!("active memory write requires non-empty keywords");
                }
                self.ensure_unique_active_keywords(&normalized_path, &normalized_keywords)?;
            }
            MemoryType::Passive => {
                if !normalized_keywords.is_empty() {
                    bail!("passive memory write must not include keywords");
                }
            }
        }

        let document_path = self.type_root(request.memory_type).join(&normalized_path);
        if let Some(parent) = document_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directories for memory file {}",
                    document_path.display()
                )
            })?;
        }

        let now = Utc::now();
        let created_at = if document_path.exists() {
            self.get(request.memory_type, &normalized_path)
                .map(|document| document.metadata.created_at)
                .unwrap_or(now)
        } else {
            now
        };
        let metadata = MemoryDocumentMetadata {
            title: title.to_string(),
            created_at,
            updated_at: now,
            keywords: normalized_keywords,
        };
        let rendered = render_memory_markdown(&metadata, request.memory_type, &request.content)?;
        fs::write(&document_path, rendered)
            .with_context(|| format!("failed to write memory file {}", document_path.display()))?;

        info!(
            memory_type = request.memory_type.as_dir_name(),
            path = %normalized_path,
            keyword_count = metadata.keywords.len(),
            root = %self.memory_root().display(),
            "wrote memory document"
        );

        Ok(MemoryDocument {
            memory_type: request.memory_type,
            path: normalized_path,
            metadata,
            content: request.content,
        })
    }

    /// List structured memory candidates, optionally filtered by type or directory prefix.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::{MemoryRepository, MemoryType};
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _items = repository.list(Some(MemoryType::Active), Some("workflow"));
    /// ```
    pub fn list(
        &self,
        memory_type: Option<MemoryType>,
        dir: Option<&str>,
    ) -> Result<Vec<MemoryDocumentSummary>> {
        let normalized_dir = match dir {
            Some(dir) => Some(validate_memory_dir_prefix(dir)?),
            None => None,
        };

        let mut documents = self.load_documents(memory_type)?;
        if let Some(dir) = normalized_dir.as_deref() {
            documents.retain(|document| document.path.starts_with(dir));
        }
        let mut items = documents
            .into_iter()
            .map(|document| summary_from_document(&document))
            .collect::<Vec<_>>();
        items.sort_by(summary_sort_key);
        Ok(items)
    }

    /// Search memory titles, keywords, paths, and body text with simple lexical matching.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::{MemoryRepository, MemoryType};
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _response = repository.search("notion", Some(MemoryType::Active), 5);
    /// ```
    pub fn search(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<MemorySearchResponse> {
        let query = query.trim();
        if query.is_empty() {
            bail!("memory search query must not be blank");
        }
        if limit == 0 {
            bail!("memory search limit must be greater than 0");
        }

        let terms = tokenize_query(query);
        let mut matches = self
            .load_documents(memory_type)?
            .into_iter()
            .filter_map(|document| {
                let score = lexical_match_score(&document, &terms);
                (score > 0).then_some((score, document))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            let left_summary = summary_from_document(&left.1);
            let right_summary = summary_from_document(&right.1);
            right
                .0
                .cmp(&left.0)
                .then_with(|| summary_sort_key(&left_summary, &right_summary))
        });

        let total_matches = matches.len();
        let items = matches
            .into_iter()
            .take(limit)
            .map(|(_, document)| summary_from_document(&document))
            .collect::<Vec<_>>();
        Ok(MemorySearchResponse {
            query: query.to_string(),
            total_matches,
            items,
        })
    }

    /// Load the active keyword catalog used during thread initialization.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::MemoryRepository;
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _catalog = repository.load_active_catalog();
    /// ```
    pub fn load_active_catalog(&self) -> Result<Vec<ActiveMemoryCatalogEntry>> {
        let documents = self.load_documents(Some(MemoryType::Active))?;
        let mut keyword_to_path = HashMap::<String, String>::new();
        let mut entries = Vec::new();

        for document in documents {
            for keyword in &document.metadata.keywords {
                let dedup_key = keyword.to_ascii_lowercase();
                if let Some(existing_path) = keyword_to_path.get(&dedup_key) {
                    bail!(
                        "duplicate active memory keyword `{keyword}` found in `{}` and `{}`",
                        existing_path,
                        document.path
                    );
                }
                keyword_to_path.insert(dedup_key, document.path.clone());
                entries.push(ActiveMemoryCatalogEntry {
                    keyword: keyword.clone(),
                    path: document.path.clone(),
                });
            }
        }

        entries.sort_by(|left, right| {
            left.keyword
                .cmp(&right.keyword)
                .then_with(|| left.path.cmp(&right.path))
        });
        info!(
            root = %self.memory_root().display(),
            entry_count = entries.len(),
            "loaded active memory catalog"
        );
        Ok(entries)
    }

    /// Build the thread-init active-memory catalog prompt when entries are available.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::memory::MemoryRepository;
    ///
    /// let repository = MemoryRepository::new("/tmp/openjarvis-workspace");
    /// let _prompt = repository.active_catalog_prompt();
    /// ```
    pub fn active_catalog_prompt(&self) -> Result<Option<String>> {
        let entries = self.load_active_catalog()?;
        if entries.is_empty() {
            return Ok(None);
        }

        let catalog = entries
            .iter()
            .map(|entry| format!("- {} -> {}", entry.keyword, entry.path))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Some(format!("{ACTIVE_MEMORY_TOOLSET_HINT}\n{catalog}")))
    }

    fn load_documents(&self, memory_type: Option<MemoryType>) -> Result<Vec<MemoryDocument>> {
        let mut documents = Vec::new();
        match memory_type {
            Some(memory_type) => {
                let type_root = self.type_root(memory_type);
                collect_markdown_documents(memory_type, &type_root, &type_root, &mut documents)?;
            }
            None => {
                for memory_type in MemoryType::iter() {
                    let type_root = self.type_root(memory_type);
                    collect_markdown_documents(
                        memory_type,
                        &type_root,
                        &type_root,
                        &mut documents,
                    )?;
                }
            }
        }

        info!(
            root = %self.memory_root().display(),
            document_count = documents.len(),
            "loaded memory documents from filesystem"
        );
        Ok(documents)
    }

    fn ensure_unique_active_keywords(&self, path: &str, keywords: &[String]) -> Result<()> {
        let existing_documents = self.load_documents(Some(MemoryType::Active))?;
        for document in existing_documents {
            if document.path == path {
                continue;
            }
            for keyword in keywords {
                if document
                    .metadata
                    .keywords
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(keyword))
                {
                    bail!(
                        "duplicate active memory keyword `{keyword}` already exists in `{}`",
                        document.path
                    );
                }
            }
        }
        Ok(())
    }
}

fn collect_markdown_documents(
    memory_type: MemoryType,
    root: &Path,
    current: &Path,
    documents: &mut Vec<MemoryDocument>,
) -> Result<()> {
    if !current.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(current)
        .with_context(|| format!("failed to read memory directory {}", current.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read one entry under memory directory {}",
                current.display()
            )
        })?;
        let path = entry.path();
        let entry_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect path {}", path.display()))?;
        if entry_type.is_dir() {
            collect_markdown_documents(memory_type, root, &path, documents)?;
            continue;
        }
        if !entry_type.is_file() {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            warn!(path = %path.display(), "ignoring non-markdown memory file");
            continue;
        }

        let relative_path = path.strip_prefix(root).with_context(|| {
            format!("failed to compute relative memory path {}", path.display())
        })?;
        let normalized_path = relative_path_to_string(relative_path)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read memory file {}", path.display()))?;
        documents.push(parse_memory_document(memory_type, &normalized_path, &raw)?);
    }

    Ok(())
}

fn parse_memory_document(memory_type: MemoryType, path: &str, raw: &str) -> Result<MemoryDocument> {
    let (frontmatter, content) = split_frontmatter(raw)?;
    let content = content.strip_suffix('\n').unwrap_or(content);
    let parsed = serde_yaml::from_str::<MemoryFrontmatter>(frontmatter)
        .with_context(|| format!("failed to parse memory frontmatter for `{path}`"))?;
    let title = parsed.title.trim();
    if title.is_empty() {
        bail!("memory document `{path}` must contain a non-empty title");
    }

    let keywords = normalize_keywords(parsed.keywords)?;
    match memory_type {
        MemoryType::Active => {
            if keywords.is_empty() {
                bail!("active memory document `{path}` must declare non-empty keywords");
            }
        }
        MemoryType::Passive => {
            if !keywords.is_empty() {
                bail!("passive memory document `{path}` must not declare keywords");
            }
        }
    }

    Ok(MemoryDocument {
        memory_type,
        path: path.to_string(),
        metadata: MemoryDocumentMetadata {
            title: title.to_string(),
            created_at: parsed.created_at,
            updated_at: parsed.updated_at,
            keywords,
        },
        content: content.to_string(),
    })
}

fn render_memory_markdown(
    metadata: &MemoryDocumentMetadata,
    memory_type: MemoryType,
    content: &str,
) -> Result<String> {
    let frontmatter = MemoryFrontmatter {
        title: metadata.title.clone(),
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        keywords: match memory_type {
            MemoryType::Active => metadata.keywords.clone(),
            MemoryType::Passive => Vec::new(),
        },
    };
    let yaml =
        serde_yaml::to_string(&frontmatter).context("failed to render memory frontmatter")?;
    let yaml = yaml.trim().to_string();
    let body = content.trim_end_matches('\n');
    Ok(format!("---\n{yaml}\n---\n{body}\n"))
}

fn split_frontmatter(raw: &str) -> Result<(&str, &str)> {
    let Some(stripped) = raw.strip_prefix("---\n") else {
        bail!("memory markdown document must start with YAML frontmatter fence");
    };
    let Some(index) = stripped.find("\n---\n") else {
        bail!("memory markdown document must close YAML frontmatter fence");
    };
    let frontmatter = &stripped[..index];
    let body = &stripped[(index + "\n---\n".len())..];
    Ok((frontmatter, body))
}

fn normalize_keywords(keywords: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = Vec::<String>::new();
    let mut seen = HashMap::<String, ()>::new();
    for keyword in keywords {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            bail!("memory keywords must not contain blank values");
        }
        let dedup_key = keyword.to_ascii_lowercase();
        if seen.insert(dedup_key, ()).is_some() {
            bail!("memory keywords must be unique within one document");
        }
        normalized.push(keyword.to_string());
    }
    Ok(normalized)
}

fn validate_memory_file_path(path: &str) -> Result<String> {
    let normalized = normalize_relative_path(path)?;
    if !normalized.ends_with(".md") {
        bail!("memory path must end with `.md`");
    }
    Ok(normalized)
}

fn validate_memory_dir_prefix(dir: &str) -> Result<String> {
    normalize_relative_path(dir)
}

fn normalize_relative_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("memory path must not be blank");
    }

    let normalized = trimmed.replace('\\', "/");
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        bail!("memory path must be relative");
    }

    let mut parts = Vec::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let value = part
                    .to_str()
                    .with_context(|| format!("memory path contains invalid utf-8: {trimmed}"))?;
                if value.trim().is_empty() {
                    bail!("memory path contains an empty component");
                }
                parts.push(value.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => bail!("memory path must not contain `..`"),
            Component::RootDir | Component::Prefix(_) => bail!("memory path must be relative"),
        }
    }

    if parts.is_empty() {
        bail!("memory path must contain at least one non-empty component");
    }
    Ok(parts.join("/"))
}

fn relative_path_to_string(path: &Path) -> Result<String> {
    let value = path
        .iter()
        .map(|part| {
            part.to_str()
                .with_context(|| format!("memory path contains invalid utf-8: {}", path.display()))
                .map(str::to_string)
        })
        .collect::<Result<Vec<_>>>()?;
    if value.is_empty() {
        bail!("memory path must not be empty");
    }
    Ok(value.join("/"))
}

fn summary_from_document(document: &MemoryDocument) -> MemoryDocumentSummary {
    MemoryDocumentSummary {
        memory_type: document.memory_type,
        path: document.path.clone(),
        title: document.metadata.title.clone(),
        updated_at: document.metadata.updated_at,
        keywords: document.metadata.keywords.clone(),
    }
}

fn summary_sort_key(
    left: &MemoryDocumentSummary,
    right: &MemoryDocumentSummary,
) -> std::cmp::Ordering {
    left.memory_type
        .cmp(&right.memory_type)
        .then_with(|| left.path.cmp(&right.path))
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|char: char| char.is_whitespace() || char == ',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn lexical_match_score(document: &MemoryDocument, terms: &[String]) -> usize {
    let searchable = format!(
        "{}\n{}\n{}\n{}\n{}",
        document.path,
        document.metadata.title,
        document.metadata.keywords.join(" "),
        document.content,
        document.memory_type.as_dir_name(),
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| searchable.contains(term.as_str()))
        .count()
}
