//! Workspace-local skill path resolution and curated skill installation helpers.

use crate::{agent::SkillManifest, cli::SkillCommand};
use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use std::{
    env::{current_dir, var},
    fs,
    path::{Path, PathBuf},
};
use tracing::{info, warn};
use uuid::Uuid;

/// Stable workspace-relative directory that stores locally installed skills.
pub const WORKSPACE_SKILL_ROOT_RELATIVE: &str = ".openjarvis/skills";
const ACPX_SKILL_PATH_OVERRIDE_ENV: &str = "OPENJARVIS_CURATED_SKILL_ACPX_PATH";
const SKILL_ENTRY_FILE_NAME: &str = "SKILL.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CuratedSkillDefinition {
    name: &'static str,
    skill_url: &'static str,
    skill_path_override_env: Option<&'static str>,
}

const ACPX_SKILL_DEFINITION: CuratedSkillDefinition = CuratedSkillDefinition {
    name: "acpx",
    skill_url: "https://raw.githubusercontent.com/openclaw/acpx/main/skills/acpx/SKILL.md",
    skill_path_override_env: Some(ACPX_SKILL_PATH_OVERRIDE_ENV),
};

/// Result snapshot returned after one curated skill installation completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub skill_name: String,
    pub skill_dir: PathBuf,
    pub skill_file: PathBuf,
    pub source_location: String,
    pub replaced_existing: bool,
}

/// Result snapshot returned after one local skill uninstall completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstalledSkill {
    pub skill_name: String,
    pub skill_dir: PathBuf,
}

/// Return the workspace-local skill root under one explicit workspace directory.
///
/// # 示例
/// ```rust
/// use openjarvis::skill::workspace_skill_root_for;
/// use std::path::Path;
///
/// let root = workspace_skill_root_for(Path::new("/tmp/openjarvis-demo"));
/// assert!(root.ends_with(".openjarvis/skills"));
/// ```
pub fn workspace_skill_root_for(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root.as_ref().join(WORKSPACE_SKILL_ROOT_RELATIVE)
}

/// Return the default skill roots for one explicit workspace directory.
///
/// # 示例
/// ```rust
/// use openjarvis::skill::default_skill_roots_for_workspace;
/// use std::path::Path;
///
/// let roots = default_skill_roots_for_workspace(Path::new("/tmp/openjarvis-demo"));
/// assert_eq!(roots.len(), 1);
/// assert!(roots[0].ends_with(".openjarvis/skills"));
/// ```
pub fn default_skill_roots_for_workspace(workspace_root: impl AsRef<Path>) -> Vec<PathBuf> {
    vec![workspace_skill_root_for(workspace_root)]
}

/// Return the default skill roots for the current workspace.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::skill::default_skill_roots;
///
/// let roots = default_skill_roots();
/// assert_eq!(roots.len(), 1);
/// ```
pub fn default_skill_roots() -> Vec<PathBuf> {
    default_skill_roots_for_workspace(resolve_current_workspace_root())
}

/// Install one curated skill by name into the provided workspace.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use openjarvis::skill::install_curated_skill;
/// use std::path::Path;
///
/// let installed = install_curated_skill("acpx", Path::new(".")).await?;
/// assert_eq!(installed.skill_name, "acpx");
/// # Ok(())
/// # }
/// ```
pub async fn install_curated_skill(
    skill_name: &str,
    workspace_root: impl AsRef<Path>,
) -> Result<InstalledSkill> {
    let definition = curated_skill_definition(skill_name)?;
    let workspace_root = workspace_root.as_ref().to_path_buf();
    let workspace_skill_root = workspace_skill_root_for(&workspace_root);
    info!(
        skill_name = definition.name,
        skill_url = definition.skill_url,
        skill_root = %workspace_skill_root.display(),
        "installing curated local skill"
    );
    let (source_location, skill_body) = load_curated_skill_body(definition).await?;
    let mut installed =
        install_curated_skill_from_contents(definition.name, &workspace_root, &skill_body)?;
    installed.source_location = source_location;
    Ok(installed)
}

async fn load_curated_skill_body(definition: CuratedSkillDefinition) -> Result<(String, String)> {
    if let Some(skill_path_override_env) = definition.skill_path_override_env {
        if let Ok(skill_path_override) = var(skill_path_override_env) {
            let normalized_skill_path_override = skill_path_override.trim();
            if !normalized_skill_path_override.is_empty() {
                info!(
                    skill_name = definition.name,
                    env_key = skill_path_override_env,
                    skill_path = normalized_skill_path_override,
                    "loading curated skill from local override path"
                );
                let skill_body =
                    fs::read_to_string(normalized_skill_path_override).with_context(|| {
                        format!(
                            "failed to read curated skill `{}` from override path `{}`",
                            definition.name, normalized_skill_path_override
                        )
                    })?;
                return Ok((normalized_skill_path_override.to_string(), skill_body));
            }
        }
    }

    let response = reqwest::get(definition.skill_url)
        .await
        .with_context(|| format!("failed to download curated skill `{}`", definition.name))?;
    let status = response.status();
    if status != StatusCode::OK {
        bail!(
            "failed to download curated skill `{}` from `{}`: http {}",
            definition.name,
            definition.skill_url,
            status
        );
    }

    let skill_body = response.text().await.with_context(|| {
        format!(
            "failed to read response body for curated skill `{}`",
            definition.name
        )
    })?;

    Ok((definition.skill_url.to_string(), skill_body))
}

/// Install one curated skill from already downloaded content into the provided workspace.
///
/// This helper exists so UT can verify install semantics without relying on the network.
pub fn install_curated_skill_from_contents(
    skill_name: &str,
    workspace_root: impl AsRef<Path>,
    skill_body: &str,
) -> Result<InstalledSkill> {
    let definition = curated_skill_definition(skill_name)?;
    let workspace_root = workspace_root.as_ref().to_path_buf();
    let skill_root = workspace_skill_root_for(&workspace_root);
    let skill_dir = skill_root.join(definition.name);
    let final_skill_file = skill_dir.join(SKILL_ENTRY_FILE_NAME);
    let temp_skill_file = skill_dir.join(format!(".skill-install-{}.tmp", Uuid::new_v4()));

    fs::create_dir_all(&skill_dir)
        .with_context(|| format!("failed to create skill dir {}", skill_dir.display()))?;
    fs::write(&temp_skill_file, skill_body).with_context(|| {
        format!(
            "failed to write temp skill file {}",
            temp_skill_file.display()
        )
    })?;

    let manifest = match SkillManifest::from_skill_file(&temp_skill_file) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = fs::remove_file(&temp_skill_file);
            return Err(error);
        }
    };

    if manifest.name != definition.name {
        let _ = fs::remove_file(&temp_skill_file);
        bail!(
            "downloaded skill manifest name `{}` does not match requested curated skill `{}`",
            manifest.name,
            definition.name
        );
    }

    let replaced_existing = final_skill_file.exists();
    if replaced_existing {
        warn!(
            skill_name = definition.name,
            skill_file = %final_skill_file.display(),
            "replacing existing local skill file during install"
        );
        fs::remove_file(&final_skill_file).with_context(|| {
            format!(
                "failed to replace existing skill file {}",
                final_skill_file.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(&temp_skill_file, &final_skill_file) {
        let _ = fs::remove_file(&temp_skill_file);
        return Err(error).with_context(|| {
            format!(
                "failed to promote temp skill file {} to {}",
                temp_skill_file.display(),
                final_skill_file.display()
            )
        });
    }

    info!(
        skill_name = definition.name,
        skill_file = %final_skill_file.display(),
        replaced_existing,
        "installed curated local skill"
    );

    Ok(InstalledSkill {
        skill_name: definition.name.to_string(),
        skill_dir,
        skill_file: final_skill_file,
        source_location: definition.skill_url.to_string(),
        replaced_existing,
    })
}

/// Uninstall one local skill by exact name from the provided workspace.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::skill::{uninstall_local_skill, workspace_skill_root_for};
/// use std::{fs, path::Path};
///
/// let workspace_root = Path::new("/tmp/openjarvis-demo-uninstall");
/// let skill_dir = workspace_skill_root_for(workspace_root).join("acpx");
/// fs::create_dir_all(&skill_dir).expect("skill dir should be created");
/// fs::write(skill_dir.join("SKILL.md"), "---\nname: acpx\ndescription: demo\n---\n")
///     .expect("skill file should be created");
///
/// let removed = uninstall_local_skill("acpx", workspace_root).expect("skill should uninstall");
/// assert_eq!(removed.skill_name, "acpx");
/// assert!(!removed.skill_dir.exists());
/// ```
pub fn uninstall_local_skill(
    skill_name: &str,
    workspace_root: impl AsRef<Path>,
) -> Result<UninstalledSkill> {
    let normalized_skill_name = normalized_skill_name(skill_name)?;
    let workspace_root = workspace_root.as_ref().to_path_buf();
    let skill_root = workspace_skill_root_for(&workspace_root);
    let skill_dir = skill_root.join(normalized_skill_name);

    if !skill_dir.exists() {
        bail!(
            "local skill `{}` is not installed under `{}`",
            normalized_skill_name,
            skill_root.display()
        );
    }

    info!(
        skill_name = normalized_skill_name,
        skill_dir = %skill_dir.display(),
        "uninstalling local skill"
    );
    if skill_dir.is_dir() {
        fs::remove_dir_all(&skill_dir)
            .with_context(|| format!("failed to remove skill dir {}", skill_dir.display()))?;
    } else {
        fs::remove_file(&skill_dir)
            .with_context(|| format!("failed to remove skill file {}", skill_dir.display()))?;
    }
    info!(
        skill_name = normalized_skill_name,
        skill_dir = %skill_dir.display(),
        "uninstalled local skill"
    );

    Ok(UninstalledSkill {
        skill_name: normalized_skill_name.to_string(),
        skill_dir,
    })
}

/// Run one top-level `openjarvis skill ...` command.
pub async fn run_cli_command(command: &SkillCommand) -> Result<()> {
    match command {
        SkillCommand::Install { name } => {
            let workspace_root = resolve_current_workspace_root();
            let installed = install_curated_skill(name, &workspace_root).await?;
            println!(
                "Installed curated skill `{}` to `{}` from `{}`.",
                installed.skill_name,
                installed.skill_file.display(),
                installed.source_location
            );
            Ok(())
        }
        SkillCommand::Uninstall { name } => {
            let workspace_root = resolve_current_workspace_root();
            let removed = uninstall_local_skill(name, &workspace_root)?;
            println!(
                "Uninstalled local skill `{}` from `{}`.",
                removed.skill_name,
                removed.skill_dir.display()
            );
            Ok(())
        }
    }
}

fn resolve_current_workspace_root() -> PathBuf {
    match current_dir() {
        Ok(path) => path,
        Err(error) => {
            warn!(
                error = %error,
                "failed to resolve current workspace root; falling back to `.`"
            );
            PathBuf::from(".")
        }
    }
}

fn curated_skill_definition(skill_name: &str) -> Result<CuratedSkillDefinition> {
    let normalized_skill_name = normalized_skill_name(skill_name)?;

    match normalized_skill_name {
        "acpx" => Ok(ACPX_SKILL_DEFINITION),
        _ => bail!(
            "unsupported curated skill `{}`; supported skills: acpx",
            normalized_skill_name
        ),
    }
}

fn normalized_skill_name(skill_name: &str) -> Result<&str> {
    let normalized_skill_name = skill_name.trim();
    if normalized_skill_name.is_empty() {
        bail!("skill command requires a non-empty skill name");
    }

    Ok(normalized_skill_name)
}
