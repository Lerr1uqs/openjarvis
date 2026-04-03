use anyhow::Result;
use async_trait::async_trait;
use openjarvis::{
    agent::{
        AutoCompactor, FeaturePromptRebuilder, ToolCallRequest, ToolCallResult, ToolDefinition,
        ToolHandler, ToolRegistry, ToolSource, ToolsetCatalogEntry, empty_tool_input_schema,
    },
    compact::ContextBudgetReport,
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole, ContextTokenKind},
    thread::{ThreadContext, ThreadContextLocator},
};
use std::{collections::HashMap, sync::Arc};

use super::tool::skill::SkillFixture;

struct DemoFeatureTool;

#[async_trait]
impl ToolHandler for DemoFeatureTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo from feature provider tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "ok".to_string(),
            metadata: serde_json::json!({}),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn feature_prompt_rebuilder_rebuilds_fixed_slots_from_all_providers() {
    // 测试场景: rebuilder 只负责写入静态 features_system_prompt；
    // 动态 context capacity 由 AutoCompactor 单独注入 transient runtime system message。
    let fixture = SkillFixture::new("openjarvis-feature-rebuilder");
    fixture.write_skill(
        "demo_skill",
        r#"---
name: demo_skill
description: help with feature rebuild tests
---
Read `guide.md` before replying.
"#,
    );
    fixture.write_skill_file("demo_skill", "guide.md", "guide content");

    let registry = Arc::new(ToolRegistry::with_skill_roots(vec![
        fixture.skills_root().to_path_buf(),
    ]));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo toolset for feature prompts"),
            vec![Arc::new(DemoFeatureTool)],
        )
        .await
        .expect("demo toolset should register");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools and skills should register");

    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let rebuilder = FeaturePromptRebuilder::new(Arc::clone(&registry), compact_config.clone());
    let auto_compactor = AutoCompactor::new(compact_config);
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_feature", "thread_feature"),
        now,
    );
    assert!(thread_context.load_toolset("demo"));
    let budget_report = ContextBudgetReport::new(
        HashMap::from([
            (ContextTokenKind::System, 24),
            (ContextTokenKind::Chat, 180),
            (ContextTokenKind::VisibleTool, 16),
            (ContextTokenKind::ReservedOutput, 16),
        ]),
        256,
    );

    rebuilder
        .rebuild(&mut thread_context, true)
        .await
        .expect("feature prompts should rebuild");
    auto_compactor.notify_capacity(&mut thread_context, Some(&budget_report));

    let features_system_prompt = thread_context.features_system_prompt();
    assert!(
        features_system_prompt
            .toolset_catalog
            .iter()
            .any(|message| message.content.contains("OpenJarvis tool-use mode"))
    );
    assert!(
        features_system_prompt
            .toolset_catalog
            .iter()
            .any(|message| message.content.contains("Available toolsets"))
    );
    assert!(
        features_system_prompt
            .skill_catalog
            .iter()
            .any(|message| message.content.contains("Available local skills"))
    );
    assert_eq!(features_system_prompt.auto_compact.len(), 1);
    assert!(
        features_system_prompt.auto_compact[0]
            .content
            .contains("Auto-compact 已开启")
    );

    let exported_messages = thread_context.messages();
    let auto_compact_index = exported_messages
        .iter()
        .position(|message| message.content.contains("Auto-compact 已开启"))
        .expect("stable auto-compact prompt should exist");
    let capacity_index = exported_messages
        .iter()
        .position(|message| message.content.contains("<context capacity"))
        .expect("dynamic capacity prompt should exist");

    assert!(auto_compact_index < capacity_index);
}

#[tokio::test]
async fn feature_prompt_rebuilder_only_updates_live_feature_slots() {
    // 测试场景: provider rebuild 只能更新 features_system_prompt，不能改写 persisted history。
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let rebuilder = FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
    );
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_feature_history",
            "thread_feature_history",
        ),
        now,
    );
    thread_context.store_turn(
        None,
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "persisted history",
            now,
        )],
        now,
        now,
    );

    rebuilder
        .rebuild(&mut thread_context, false)
        .await
        .expect("feature prompts should rebuild");

    assert_eq!(thread_context.load_messages().len(), 1);
    assert_eq!(
        thread_context.load_messages()[0].content,
        "persisted history"
    );
    assert!(
        thread_context
            .features_system_prompt()
            .toolset_catalog
            .iter()
            .any(|message| message.content.contains("OpenJarvis tool-use mode"))
    );
    assert!(
        thread_context
            .features_system_prompt()
            .auto_compact
            .is_empty()
    );
    assert!(
        !thread_context
            .messages()
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
}

#[tokio::test]
async fn auto_compactor_injects_capacity_as_transient_runtime_system_message() {
    // 测试场景: 动态上下文容量提示不应占用固定 slot，而应由 AutoCompactor 作为 transient system message 注入。
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let auto_compactor = AutoCompactor::new(compact_config.clone());
    let now = chrono::Utc::now();
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_auto_compactor",
            "thread_auto_compactor",
        ),
        now,
    );
    let budget_report = ContextBudgetReport::new(
        HashMap::from([
            (ContextTokenKind::System, 32),
            (ContextTokenKind::Memory, 8),
            (ContextTokenKind::Chat, 160),
            (ContextTokenKind::VisibleTool, 16),
            (ContextTokenKind::ReservedOutput, 16),
        ]),
        256,
    );

    auto_compactor.notify_capacity(&mut thread_context, Some(&budget_report));

    assert!(
        thread_context
            .features_system_prompt()
            .auto_compact
            .is_empty()
    );
    assert!(
        thread_context
            .messages()
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
}
