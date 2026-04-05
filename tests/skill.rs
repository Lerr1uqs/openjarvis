use openjarvis::{
    agent::{SkillManifest, SkillRegistry},
    skill::{
        default_skill_roots_for_workspace, install_curated_skill_from_contents,
        uninstall_local_skill, workspace_skill_root_for,
    },
};
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

struct SkillInstallFixture {
    root: PathBuf,
}

impl SkillInstallFixture {
    fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("skill fixture root should be created");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn installed_skill_file(&self, skill_name: &str) -> PathBuf {
        workspace_skill_root_for(&self.root)
            .join(skill_name)
            .join("SKILL.md")
    }
}

impl Drop for SkillInstallFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn acpx_skill_resource_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/unittest/skills/acpx/SKILL.md")
}

fn acpx_skill_resource_body() -> String {
    fs::read_to_string(acpx_skill_resource_path()).expect("acpx skill fixture should be readable")
}

#[test]
fn default_skill_roots_for_workspace_uses_dot_openjarvis_skills() {
    // 测试场景: 默认 skill roots 应与工作区 `.openjarvis/skills` 绑定，而不是旧的 `.skills`。
    let fixture = SkillInstallFixture::new("openjarvis-skill-default-root");
    let roots = default_skill_roots_for_workspace(fixture.root());

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0], workspace_skill_root_for(fixture.root()));
    assert!(roots[0].ends_with(".openjarvis/skills"));
}

#[test]
fn curated_acpx_skill_resource_uses_expected_skill_file_format() {
    // 测试场景: vendored acpx skill fixture 必须保持 `SKILL.md` 文件名和合法 frontmatter。
    let skill_file = acpx_skill_resource_path();

    assert_eq!(
        skill_file.file_name().and_then(|name| name.to_str()),
        Some("SKILL.md")
    );
    let manifest =
        SkillManifest::from_skill_file(&skill_file).expect("acpx skill fixture should parse");
    assert_eq!(manifest.name, "acpx");
}

#[tokio::test]
async fn install_curated_skill_from_contents_writes_valid_skill_into_workspace_root() {
    // 测试场景: 安装器应把合法 curated skill 写入工作区 `.openjarvis/skills/<name>/SKILL.md`
    // 并让默认 registry 能直接发现该 skill。
    let fixture = SkillInstallFixture::new("openjarvis-skill-install-valid");
    let installed =
        install_curated_skill_from_contents("acpx", fixture.root(), &acpx_skill_resource_body())
            .expect("curated skill install should succeed");

    assert_eq!(installed.skill_name, "acpx");
    assert_eq!(installed.skill_file, fixture.installed_skill_file("acpx"));
    assert!(installed.skill_file.exists());

    let manifest = SkillManifest::from_skill_file(&installed.skill_file)
        .expect("installed skill should parse");
    assert_eq!(manifest.name, "acpx");

    let registry = SkillRegistry::with_roots(vec![workspace_skill_root_for(fixture.root())]);
    let manifests = registry
        .reload()
        .await
        .expect("registry reload should succeed");
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].name, "acpx");
}

#[test]
fn install_curated_skill_from_contents_overwrites_existing_skill_file() {
    // 测试场景: 重复 install 应覆盖旧 skill 文件，避免没有 update 命令时无法刷新内容。
    let fixture = SkillInstallFixture::new("openjarvis-skill-install-overwrite");
    let first = install_curated_skill_from_contents(
        "acpx",
        fixture.root(),
        r#"---
name: acpx
description: first version
---
v1
"#,
    )
    .expect("first install should succeed");
    let second = install_curated_skill_from_contents(
        "acpx",
        fixture.root(),
        r#"---
name: acpx
description: second version
---
v2
"#,
    )
    .expect("second install should succeed");

    assert!(!first.replaced_existing);
    assert!(second.replaced_existing);
    let installed_content = fs::read_to_string(fixture.installed_skill_file("acpx"))
        .expect("installed file should exist");
    assert!(installed_content.contains("second version"));
    assert!(installed_content.contains("v2"));
}

#[test]
fn uninstall_local_skill_removes_installed_skill_directory() {
    // 测试场景: uninstall 应删除目标 skill 目录和 `SKILL.md`，避免旧 skill 残留。
    let fixture = SkillInstallFixture::new("openjarvis-skill-uninstall");
    install_curated_skill_from_contents("acpx", fixture.root(), &acpx_skill_resource_body())
        .expect("curated skill install should succeed before uninstall");

    let removed = uninstall_local_skill("acpx", fixture.root())
        .expect("installed skill should uninstall successfully");

    assert_eq!(removed.skill_name, "acpx");
    assert!(!removed.skill_dir.exists());
    assert!(!fixture.installed_skill_file("acpx").exists());
}

#[test]
fn install_curated_skill_from_contents_rejects_invalid_manifest_without_final_file() {
    // 测试场景: 远端返回非法 SKILL.md 时，安装器不能留下损坏的最终文件。
    let fixture = SkillInstallFixture::new("openjarvis-skill-install-invalid");
    let error = install_curated_skill_from_contents("acpx", fixture.root(), "broken skill body")
        .expect_err("invalid skill should fail");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("skill file must start with YAML frontmatter"));
    assert!(!fixture.installed_skill_file("acpx").exists());
}

#[test]
fn install_curated_skill_from_contents_rejects_mismatched_manifest_name() {
    // 测试场景: curated skill 名和下载内容中的 manifest name 不一致时应拒绝安装。
    let fixture = SkillInstallFixture::new("openjarvis-skill-install-name-mismatch");
    let error = install_curated_skill_from_contents(
        "acpx",
        fixture.root(),
        r#"---
name: not_acpx
description: mismatch
---
body
"#,
    )
    .expect_err("mismatched manifest name should fail");

    assert!(
        error
            .to_string()
            .contains("does not match requested curated skill")
    );
    assert!(!fixture.installed_skill_file("acpx").exists());
}

#[test]
fn install_curated_skill_from_contents_rejects_unknown_curated_skill_name() {
    // 测试场景: 只有 curated registry 中声明的 skill 才允许被安装。
    let fixture = SkillInstallFixture::new("openjarvis-skill-install-unknown");
    let error = install_curated_skill_from_contents(
        "missing",
        fixture.root(),
        r#"---
name: missing
description: missing
---
body
"#,
    )
    .expect_err("unknown curated skill should fail");

    assert!(
        error
            .to_string()
            .contains("unsupported curated skill `missing`")
    );
}
