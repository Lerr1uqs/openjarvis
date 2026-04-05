//! Local skill registry that scans `.openjarvis/skills`, indexes manifests, and loads skill bodies on demand.

use crate::skill::default_skill_roots;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
use tokio::sync::RwLock;
use tracing::warn;

const SKILL_ENTRY_FILE_NAME: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub skill_dir: PathBuf,
    pub skill_file: PathBuf,
    pub enabled: bool,
}

impl SkillManifest {
    /// Parse one skill manifest from a local `SKILL.md` path.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::agent::SkillManifest;
    ///
    /// let manifest = SkillManifest::from_skill_file(".openjarvis/skills/example/SKILL.md")
    ///     .expect("skill manifest should parse");
    /// assert_eq!(manifest.name, "example");
    /// ```
    pub fn from_skill_file(path: impl AsRef<Path>) -> Result<Self> {
        let skill_file = path.as_ref().to_path_buf();
        let raw = fs::read_to_string(&skill_file)
            .with_context(|| format!("failed to read skill file {}", skill_file.display()))?;
        let document = parse_skill_document(&raw)
            .with_context(|| format!("failed to parse skill file {}", skill_file.display()))?;
        let skill_dir = skill_file.parent().with_context(|| {
            format!(
                "skill file {} must live inside a dedicated directory",
                skill_file.display()
            )
        })?;

        Ok(Self {
            name: document.frontmatter.name,
            description: document.frontmatter.description,
            skill_dir: skill_dir.to_path_buf(),
            skill_file,
            enabled: true,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkillFile {
    pub relative_path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub body: String,
    pub referenced_files: Vec<LoadedSkillFile>,
}

impl LoadedSkill {
    /// Render the loaded skill into a prompt-friendly text block.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::{LoadedSkill, SkillManifest};
    /// use std::path::PathBuf;
    ///
    /// let prompt = LoadedSkill {
    ///     manifest: SkillManifest {
    ///         name: "demo".to_string(),
    ///         description: "demo skill".to_string(),
    ///         skill_dir: PathBuf::from(".openjarvis/skills/demo"),
    ///         skill_file: PathBuf::from(".openjarvis/skills/demo/SKILL.md"),
    ///         enabled: true,
    ///     },
    ///     body: "Do the demo task.".to_string(),
    ///     referenced_files: Vec::new(),
    /// }
    /// .to_prompt();
    ///
    /// assert!(prompt.contains("demo skill"));
    /// ```
    pub fn to_prompt(&self) -> String {
        let mut sections = vec![
            format!("Loaded local skill `{}`.", self.manifest.name),
            format!("Description: {}", self.manifest.description),
            "Use the following instructions for the current task when they are relevant."
                .to_string(),
        ];

        if self.body.trim().is_empty() {
            sections.push("SKILL.md body: (empty)".to_string());
        } else {
            sections.push(format!("SKILL.md body:\n{}", self.body));
        }

        if !self.referenced_files.is_empty() {
            let referenced_files = self
                .referenced_files
                .iter()
                .map(|file| {
                    format!(
                        "Referenced file `{}`:\n{}",
                        file.relative_path, file.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            sections.push(referenced_files);
        }

        sections.join("\n\n")
    }
}

pub struct SkillRegistry {
    state: RwLock<SkillRegistryState>,
}

impl SkillRegistry {
    /// Create a skill registry that only scans the current workspace `.openjarvis/skills`
    /// directory.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::agent::SkillRegistry;
    ///
    /// let registry = SkillRegistry::new();
    /// assert!(registry.list().await.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Self {
        Self::with_roots(default_skill_roots())
    }

    /// Create a skill registry with explicit local roots.
    pub fn with_roots(roots: Vec<PathBuf>) -> Self {
        Self {
            state: RwLock::new(SkillRegistryState {
                roots,
                manifests: BTreeMap::new(),
            }),
        }
    }

    /// Reload all local skills from disk.
    ///
    /// Invalid or duplicate skill packages are skipped and logged so one broken local skill does
    /// not block the rest of the runtime.
    pub async fn reload(&self) -> Result<Vec<SkillManifest>> {
        let (roots, previous_enabled) = {
            let state = self.state.read().await;
            let previous_enabled = state
                .manifests
                .iter()
                .map(|(name, manifest)| (name.clone(), manifest.enabled))
                .collect::<BTreeMap<_, _>>();
            (state.roots.clone(), previous_enabled)
        };

        let mut manifests = BTreeMap::new();
        for skill_file in discover_skill_files(&roots)? {
            match SkillManifest::from_skill_file(&skill_file) {
                Ok(mut manifest) => {
                    manifest.enabled = previous_enabled
                        .get(&manifest.name)
                        .copied()
                        .unwrap_or(true);
                    if manifests.contains_key(&manifest.name) {
                        warn!(
                            skill_name = %manifest.name,
                            skill_file = %manifest.skill_file.display(),
                            "duplicate local skill name detected, skipping later entry"
                        );
                        continue;
                    }
                    manifests.insert(manifest.name.clone(), manifest);
                }
                Err(error) => {
                    warn!(
                        skill_file = %skill_file.display(),
                        error = %error,
                        "failed to load local skill manifest, skipping entry"
                    );
                }
            }
        }

        let snapshots = manifests.values().cloned().collect::<Vec<_>>();
        self.state.write().await.manifests = manifests;
        Ok(snapshots)
    }

    /// List all discovered skills, including disabled entries.
    pub async fn list(&self) -> Vec<SkillManifest> {
        self.state
            .read()
            .await
            .manifests
            .values()
            .cloned()
            .collect::<Vec<_>>()
    }

    /// List all enabled skills.
    pub async fn list_enabled(&self) -> Vec<SkillManifest> {
        self.state
            .read()
            .await
            .manifests
            .values()
            .filter(|manifest| manifest.enabled)
            .cloned()
            .collect::<Vec<_>>()
    }

    /// Return whether at least one enabled local skill is available.
    pub async fn has_enabled_skills(&self) -> bool {
        self.state
            .read()
            .await
            .manifests
            .values()
            .any(|manifest| manifest.enabled)
    }

    /// Disable one local skill in memory.
    ///
    /// TODO: later convert the global installed skill set into a per-agent local enabled copy.
    pub async fn disable(&self, name: &str) -> Result<SkillManifest> {
        let mut state = self.state.write().await;
        let manifest = state
            .manifests
            .get_mut(name)
            .with_context(|| format!("local skill `{name}` does not exist"))?;
        manifest.enabled = false;
        Ok(manifest.clone())
    }

    /// Enable one local skill in memory.
    ///
    /// TODO: later convert the global installed skill set into a per-agent local enabled copy.
    pub async fn enable(&self, name: &str) -> Result<SkillManifest> {
        let mut state = self.state.write().await;
        let manifest = state
            .manifests
            .get_mut(name)
            .with_context(|| format!("local skill `{name}` does not exist"))?;
        manifest.enabled = true;
        Ok(manifest.clone())
    }

    /// Enable only the selected skills and disable every other discovered local skill.
    ///
    /// An empty selection disables all discovered skills.
    pub async fn restrict_to(&self, names: &[String]) -> Result<Vec<SkillManifest>> {
        let selected_names = names
            .iter()
            .map(|name| {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    bail!("local skill name must not be blank");
                }
                Ok(trimmed.to_string())
            })
            .collect::<Result<BTreeSet<_>>>()?;

        let discovered_manifests = self.reload().await?;
        let discovered_names = discovered_manifests
            .iter()
            .map(|manifest| manifest.name.clone())
            .collect::<BTreeSet<_>>();

        for selected_name in &selected_names {
            if !discovered_names.contains(selected_name) {
                bail!("local skill `{selected_name}` does not exist");
            }
        }

        let mut state = self.state.write().await;
        for manifest in state.manifests.values_mut() {
            manifest.enabled = selected_names.contains(&manifest.name);
        }

        Ok(state
            .manifests
            .values()
            .filter(|manifest| manifest.enabled)
            .cloned()
            .collect::<Vec<_>>())
    }

    /// Load one enabled skill by exact name.
    pub async fn load(&self, name: &str) -> Result<LoadedSkill> {
        let manifest = self
            .state
            .read()
            .await
            .manifests
            .get(name)
            .cloned()
            .with_context(|| format!("local skill `{name}` does not exist"))?;

        if !manifest.enabled {
            bail!("local skill `{name}` is disabled");
        }

        load_skill_from_manifest(&manifest)
    }

    /// Build the catalog prompt injected into the agent loop when skills are available.
    pub async fn catalog_prompt(&self) -> Option<String> {
        let enabled_manifests = self.list_enabled().await;
        if enabled_manifests.is_empty() {
            return None;
        }

        let skill_lines = enabled_manifests
            .iter()
            .map(|manifest| format!("- {}: {}", manifest.name, manifest.description))
            .collect::<Vec<_>>()
            .join("\n");

        Some(format!(
            "You have access to local skills through the `load_skill` tool.\n\
Only call `load_skill` when one of the available skills is directly relevant.\n\
Do not assume any skill body before loading it.\n\n\
Available local skills:\n{}",
            skill_lines
        ))
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct SkillRegistryState {
    roots: Vec<PathBuf>,
    manifests: BTreeMap<String, SkillManifest>,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(flatten)]
    extra_fields: BTreeMap<String, serde_yaml::Value>,
}

struct ParsedSkillDocument {
    frontmatter: SkillFrontmatter,
    body: String,
}

fn load_skill_from_manifest(manifest: &SkillManifest) -> Result<LoadedSkill> {
    let raw = fs::read_to_string(&manifest.skill_file).with_context(|| {
        format!(
            "failed to read skill file {}",
            manifest.skill_file.display()
        )
    })?;
    let document = parse_skill_document(&raw).with_context(|| {
        format!(
            "failed to parse skill file {}",
            manifest.skill_file.display()
        )
    })?;
    let referenced_files = load_referenced_files(&manifest.skill_dir, &document.body)?;

    Ok(LoadedSkill {
        manifest: manifest.clone(),
        body: document.body,
        referenced_files,
    })
}

fn parse_skill_document(raw: &str) -> Result<ParsedSkillDocument> {
    let (frontmatter_raw, body_raw) = split_frontmatter(raw)?;
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(&frontmatter_raw).context("failed to parse skill frontmatter")?;

    let name = frontmatter.name.trim();
    if name.is_empty() {
        bail!("skill frontmatter `name` must not be blank");
    }

    let description = frontmatter.description.trim();
    if description.is_empty() {
        bail!("skill frontmatter `description` must not be blank");
    }

    let _ = &frontmatter.extra_fields;

    Ok(ParsedSkillDocument {
        frontmatter: SkillFrontmatter {
            name: name.to_string(),
            description: description.to_string(),
            extra_fields: frontmatter.extra_fields,
        },
        body: body_raw.trim().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> Result<(String, &str)> {
    let mut lines = raw.split_inclusive('\n');
    let Some(first_line) = lines.next() else {
        bail!("skill file must not be empty");
    };

    if trim_markdown_line(first_line) != "---" {
        bail!("skill file must start with YAML frontmatter");
    }

    let mut offset = first_line.len();
    let mut frontmatter = String::new();

    for line in lines {
        if trim_markdown_line(line) == "---" {
            offset += line.len();
            return Ok((frontmatter, &raw[offset..]));
        }
        frontmatter.push_str(line);
        offset += line.len();
    }

    bail!("skill file frontmatter is missing a closing `---` fence")
}

fn discover_skill_files(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        walk_skill_files(root, &mut files)?;
    }

    files.sort();
    Ok(files)
}

fn walk_skill_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read skill dir {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to inspect {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type {}", path.display()))?;

        if file_type.is_dir() {
            walk_skill_files(&path, files)?;
            continue;
        }

        if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == SKILL_ENTRY_FILE_NAME)
        {
            files.push(path);
        }
    }

    Ok(())
}

fn load_referenced_files(skill_dir: &Path, body: &str) -> Result<Vec<LoadedSkillFile>> {
    let mut referenced_files = Vec::new();
    let skill_root = skill_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve skill dir {}", skill_dir.display()))?;

    for candidate in extract_reference_candidates(body) {
        if !is_relative_skill_path(&candidate) {
            continue;
        }

        let candidate_path = skill_dir.join(&candidate);
        if !candidate_path.exists() || !candidate_path.is_file() {
            continue;
        }

        let canonical_target = candidate_path.canonicalize().with_context(|| {
            format!(
                "failed to resolve referenced skill file {}",
                candidate_path.display()
            )
        })?;
        if !canonical_target.starts_with(&skill_root) {
            bail!(
                "referenced skill file {} escapes the skill directory",
                candidate_path.display()
            );
        }

        let content = fs::read_to_string(&canonical_target).with_context(|| {
            format!(
                "failed to read referenced skill file {}",
                canonical_target.display()
            )
        })?;
        let relative_path = canonical_target
            .strip_prefix(&skill_root)
            .unwrap_or(&canonical_target)
            .to_string_lossy()
            .replace('\\', "/");
        referenced_files.push(LoadedSkillFile {
            relative_path,
            content,
        });
    }

    Ok(referenced_files)
}

fn extract_reference_candidates(body: &str) -> BTreeSet<String> {
    let mut candidates = BTreeSet::new();
    extract_backtick_candidates(body, &mut candidates);
    extract_markdown_link_candidates(body, &mut candidates);
    candidates
}

fn extract_backtick_candidates(body: &str, candidates: &mut BTreeSet<String>) {
    let mut in_backticks = false;
    let mut current = String::new();

    for ch in body.chars() {
        if ch == '`' {
            if in_backticks {
                let candidate = current.trim();
                if !candidate.is_empty() {
                    candidates.insert(candidate.to_string());
                }
                current.clear();
            }
            in_backticks = !in_backticks;
            continue;
        }

        if in_backticks {
            current.push(ch);
        }
    }
}

fn extract_markdown_link_candidates(body: &str, candidates: &mut BTreeSet<String>) {
    let mut remaining = body;

    while let Some(open_index) = remaining.find("](") {
        let after_open = &remaining[(open_index + 2)..];
        let Some(close_index) = after_open.find(')') else {
            break;
        };
        let candidate = after_open[..close_index].trim();
        if !candidate.is_empty() {
            candidates.insert(candidate.to_string());
        }
        remaining = &after_open[(close_index + 1)..];
    }
}

fn is_relative_skill_path(candidate: &str) -> bool {
    if candidate.trim().is_empty()
        || candidate.starts_with('#')
        || candidate.contains("://")
        || Path::new(candidate).is_absolute()
    {
        return false;
    }

    Path::new(candidate)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn trim_markdown_line(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}
