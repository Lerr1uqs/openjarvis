use openjarvis::agent::{ToolCallRequest, ToolHandler, ToolRegistry};
use serde_json::json;
use std::sync::Arc;

use super::SkillFixture;

#[tokio::test]
async fn load_skill_tool_returns_prompt_and_metadata() {
    let fixture = SkillFixture::new("openjarvis-load-skill-tool");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: demo skill description
---
Follow `guide.md`.
"#,
    );
    fixture.write_skill_file("demo_skill", "guide.md", "guide content");

    let registry = ToolRegistry::with_skill_roots(vec![fixture.skills_root().to_path_buf()]);
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let result = registry
        .call(ToolCallRequest {
            name: "load_skill".to_string(),
            arguments: json!({
                "name": "demo_skill"
            }),
        })
        .await
        .expect("load_skill should succeed");

    assert!(!result.is_error);
    assert!(result.content.contains("Loaded local skill `demo_skill`."));
    assert!(result.content.contains("Follow `guide.md`."));
    assert!(result.content.contains("guide content"));
    assert_eq!(result.metadata["name"], "demo_skill");
    assert_eq!(result.metadata["referenced_file_count"], 1);
}

#[tokio::test]
async fn load_skill_tool_rejects_blank_name() {
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    let tool =
        openjarvis::agent::LoadSkillTool::new(Arc::new(openjarvis::agent::SkillRegistry::new()));

    let error = tool
        .call(ToolCallRequest {
            name: "load_skill".to_string(),
            arguments: json!({
                "name": "   "
            }),
        })
        .await
        .expect_err("blank skill name should fail");

    assert!(format!("{error:#}").contains("load_skill requires a non-empty `name`"));
    assert!(registry.list().await.is_empty());
}

#[tokio::test]
async fn tool_registry_only_exposes_load_skill_when_enabled_skills_exist() {
    let empty_fixture = SkillFixture::new("openjarvis-load-skill-tool-empty");
    let empty_registry =
        ToolRegistry::with_skill_roots(vec![empty_fixture.skills_root().to_path_buf()]);
    empty_registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let empty_names = empty_registry
        .list()
        .await
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(!empty_names.iter().any(|name| name == "load_skill"));

    let fixture = SkillFixture::new("openjarvis-load-skill-tool-available");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: demo skill description
---
body
"#,
    );

    let registry = ToolRegistry::with_skill_roots(vec![fixture.skills_root().to_path_buf()]);
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let names = registry
        .list()
        .await
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "load_skill"));

    registry
        .skills()
        .disable("demo_skill")
        .await
        .expect("skill should be disabled");
    let disabled_names = registry
        .list()
        .await
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(!disabled_names.iter().any(|name| name == "load_skill"));
}

#[tokio::test]
async fn tool_registry_skill_api_restricts_enabled_skills_to_selected_names() {
    let fixture = SkillFixture::new("openjarvis-load-skill-tool-restrict");
    fixture.write_skill(
        "alpha_skill",
        r#"---
name: alpha_skill
description: alpha description
---
alpha body
"#,
    );
    fixture.write_skill(
        "beta_skill",
        r#"---
name: beta_skill
description: beta description
---
beta body
"#,
    );

    let registry = ToolRegistry::with_skill_roots(vec![fixture.skills_root().to_path_buf()]);
    let enabled = registry
        .skills()
        .restrict_to(&["beta_skill".to_string()])
        .await
        .expect("skill restriction should succeed");

    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].name, "beta_skill");
    let prompt = registry
        .skills()
        .catalog_prompt()
        .await
        .expect("catalog prompt should exist");
    assert!(prompt.contains("beta_skill"));
    assert!(!prompt.contains("alpha_skill"));

    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let load_result = registry
        .call(ToolCallRequest {
            name: "load_skill".to_string(),
            arguments: json!({
                "name": "beta_skill"
            }),
        })
        .await
        .expect("selected skill should stay loadable");
    assert!(load_result.content.contains("beta body"));

    let error = registry
        .call(ToolCallRequest {
            name: "load_skill".to_string(),
            arguments: json!({
                "name": "alpha_skill"
            }),
        })
        .await
        .expect_err("non-selected skill should be disabled");
    assert!(format!("{error:#}").contains("local skill `alpha_skill` is disabled"));
}

#[tokio::test]
async fn tool_registry_skill_api_restrict_to_missing_skill_fails() {
    let fixture = SkillFixture::new("openjarvis-load-skill-tool-restrict-missing");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: demo description
---
body
"#,
    );

    let registry = ToolRegistry::with_skill_roots(vec![fixture.skills_root().to_path_buf()]);
    let error = registry
        .skills()
        .restrict_to(&["missing_skill".to_string()])
        .await
        .expect_err("missing skill should fail");

    assert!(format!("{error:#}").contains("local skill `missing_skill` does not exist"));
}
