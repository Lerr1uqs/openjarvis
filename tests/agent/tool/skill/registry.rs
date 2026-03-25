use openjarvis::agent::{SkillManifest, SkillRegistry};

use super::SkillFixture;

#[test]
fn skill_manifest_parses_valid_frontmatter() {
    let fixture = SkillFixture::new("openjarvis-skill-manifest-valid");
    let skill_file = fixture.write_skill(
        "translator",
        r#"---
name: translator
description: translate content between languages
---
Use `reference.md` before translating.
"#,
    );

    let manifest = SkillManifest::from_skill_file(&skill_file).expect("manifest should parse");

    assert_eq!(manifest.name, "translator");
    assert_eq!(manifest.description, "translate content between languages");
    assert!(manifest.enabled);
    assert_eq!(manifest.skill_file, skill_file);
    assert_eq!(
        manifest
            .skill_dir
            .file_name()
            .and_then(|name| name.to_str()),
        Some("translator")
    );
}

#[test]
fn malformed_skill_manifest_without_frontmatter_is_rejected() {
    let fixture = SkillFixture::new("openjarvis-skill-manifest-no-frontmatter");
    let skill_file = fixture.write_skill("broken", "name: broken\ndescription: missing fence\n");

    let error =
        SkillManifest::from_skill_file(&skill_file).expect_err("missing frontmatter should fail");

    assert!(
        format!("{error:#}").contains("skill file must start with YAML frontmatter"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn malformed_skill_manifest_without_closing_fence_is_rejected() {
    let fixture = SkillFixture::new("openjarvis-skill-manifest-no-closing-fence");
    let skill_file = fixture.write_skill(
        "broken",
        r#"---
name: broken
description: missing closing fence
"#,
    );

    let error = SkillManifest::from_skill_file(&skill_file)
        .expect_err("missing closing frontmatter fence should fail");

    assert!(
        format!("{error:#}").contains("skill file frontmatter is missing a closing `---` fence"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn malformed_skill_manifest_without_name_is_rejected() {
    let fixture = SkillFixture::new("openjarvis-skill-manifest-no-name");
    let skill_file = fixture.write_skill(
        "broken",
        r#"---
description: missing name
---
body
"#,
    );

    let error = SkillManifest::from_skill_file(&skill_file).expect_err("missing name should fail");
    let error_chain = format!("{error:#}");

    assert!(error_chain.contains("failed to parse skill frontmatter"));
    assert!(error_chain.contains("missing field `name`"));
}

#[tokio::test]
async fn skill_registry_reload_skips_invalid_entries_and_lists_valid_skills() {
    let fixture = SkillFixture::new("openjarvis-skill-registry-reload");
    fixture.write_skill(
        "valid_skill",
        r#"---
name: valid_skill
description: valid local skill
---
Use `guide.md`.
"#,
    );
    fixture.write_skill(
        "invalid_skill",
        r#"---
name: invalid_skill
---
missing description
"#,
    );

    let registry = SkillRegistry::with_roots(vec![fixture.skills_root().to_path_buf()]);
    let manifests = registry.reload().await.expect("reload should succeed");

    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].name, "valid_skill");
    assert_eq!(registry.list().await.len(), 1);
    assert_eq!(registry.list_enabled().await.len(), 1);
}

#[tokio::test]
async fn skill_registry_enable_and_disable_updates_catalog_prompt() {
    let fixture = SkillFixture::new("openjarvis-skill-registry-enable-disable");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: demo skill description
---
Do the demo.
"#,
    );

    let registry = SkillRegistry::with_roots(vec![fixture.skills_root().to_path_buf()]);
    registry.reload().await.expect("reload should succeed");

    let prompt = registry
        .catalog_prompt()
        .await
        .expect("catalog prompt should exist");
    assert!(prompt.contains("demo_skill"));

    let disabled = registry
        .disable("demo_skill")
        .await
        .expect("skill should be disabled");
    assert!(!disabled.enabled);
    assert!(registry.list_enabled().await.is_empty());
    assert!(registry.catalog_prompt().await.is_none());

    let enabled = registry
        .enable("demo_skill")
        .await
        .expect("skill should be enabled again");
    assert!(enabled.enabled);
    assert!(registry.catalog_prompt().await.is_some());
}

#[tokio::test]
async fn skill_registry_loads_referenced_files_without_directory_escape() {
    let fixture = SkillFixture::new("openjarvis-skill-registry-references");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: demo skill description
---
Read `guide.md` and [form](assets/form.txt). Ignore `../outside.txt`.
"#,
    );
    fixture.write_skill_file("demo_skill", "guide.md", "guide content");
    fixture.write_skill_file("demo_skill", "assets/form.txt", "form content");
    fixture.write_skill_file("demo_skill", "nested/notes.md", "unused");

    let registry = SkillRegistry::with_roots(vec![fixture.skills_root().to_path_buf()]);
    registry.reload().await.expect("reload should succeed");
    let loaded_skill = registry
        .load("demo_skill")
        .await
        .expect("skill should load");

    assert_eq!(loaded_skill.manifest.name, "demo_skill");
    assert!(loaded_skill.body.contains("Read `guide.md`"));
    assert_eq!(loaded_skill.referenced_files.len(), 2);
    assert_eq!(
        loaded_skill.referenced_files[0].relative_path,
        "assets/form.txt"
    );
    assert_eq!(loaded_skill.referenced_files[0].content, "form content");
    assert_eq!(loaded_skill.referenced_files[1].relative_path, "guide.md");
    assert_eq!(loaded_skill.referenced_files[1].content, "guide content");
    assert!(
        loaded_skill
            .to_prompt()
            .contains("Referenced file `guide.md`")
    );
    assert!(
        !loaded_skill
            .to_prompt()
            .contains("Referenced file `../outside.txt`")
    );
}
