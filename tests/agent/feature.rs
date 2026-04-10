use super::support::ThreadTestExt;
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
    thread::{Thread, ThreadContextLocator, ThreadRuntimeAttachment},
};
use std::{collections::HashMap, fs, path::Path, sync::Arc};

use super::tool::skill::SkillFixture;

struct DemoFeatureTool;

fn acpx_skill_resource_body() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/unittest/skills/acpx/SKILL.md"),
    )
    .expect("acpx skill fixture should be readable")
}

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

fn build_runtime_attachment(
    registry: Arc<ToolRegistry>,
    system_prompt: &str,
) -> ThreadRuntimeAttachment {
    let rebuilder = Arc::new(FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
        system_prompt,
    ));
    let memory_repository = registry.memory_repository();
    ThreadRuntimeAttachment::new(registry, memory_repository, rebuilder, false)
}

#[tokio::test]
async fn feature_prompt_rebuilder_rebuilds_fixed_slots_from_all_providers() {
    // 测试场景: rebuilder 应生成稳定 system messages，
    // 并由调用方在 init 时一次性持久化进 Thread。
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
    let rebuilder = FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        compact_config.clone(),
        "worker system prompt",
    );
    let auto_compactor = AutoCompactor::new(compact_config);
    let now = chrono::Utc::now();
    let mut thread_context = Thread::new(
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

    let built_messages = rebuilder
        .build_messages(&thread_context, true)
        .await
        .expect("feature prompts should build");
    thread_context.seed_persisted_messages(built_messages);
    assert!(auto_compactor.compact_tool_visible(&budget_report));
    assert!(
        !auto_compactor.runtime_compaction_required(&ContextBudgetReport::new(
            HashMap::from([
                (ContextTokenKind::System, 24),
                (ContextTokenKind::Chat, 80),
                (ContextTokenKind::VisibleTool, 16),
                (ContextTokenKind::ReservedOutput, 16),
            ]),
            256,
        ))
    );

    assert!(
        thread_context
            .system_messages()
            .iter()
            .any(|message| message.content.contains("worker system prompt"))
    );
    assert!(
        thread_context
            .system_messages()
            .iter()
            .any(|message| message.content.contains("OpenJarvis tool-use mode"))
    );
    assert!(
        thread_context
            .system_messages()
            .iter()
            .any(|message| message.content.contains("Available toolsets"))
    );
    assert!(
        thread_context
            .system_messages()
            .iter()
            .any(|message| message.content.contains("Available local skills"))
    );
    assert!(
        thread_context
            .system_messages()
            .iter()
            .any(|message| message.content.contains("Auto-compact 已开启"))
    );

    assert!(
        !thread_context
            .messages()
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
}

#[tokio::test]
async fn feature_prompt_rebuilder_includes_enabled_acpx_skill_in_thread_system_message() {
    // 测试场景: 启动阶段如果显式启用 `acpx`，thread 固定 system message 中应出现该 skill。
    let fixture = SkillFixture::new("openjarvis-feature-acpx-skill");
    fixture.write_skill("acpx", &acpx_skill_resource_body());

    let registry = Arc::new(ToolRegistry::with_skill_roots(vec![
        fixture.skills_root().to_path_buf(),
    ]));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools and skills should register");
    registry
        .skills()
        .restrict_to(&["acpx".to_string()])
        .await
        .expect("acpx should be enabled");

    let rebuilder = FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
        "",
    );
    let now = chrono::Utc::now();
    let mut thread_context = Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_acpx", "thread_acpx"),
        now,
    );

    let built_messages = rebuilder
        .build_messages(&thread_context, false)
        .await
        .expect("feature prompts should build");

    thread_context.seed_persisted_messages(built_messages);
    let system_messages = thread_context.system_messages();
    let skill_message = system_messages
        .iter()
        .find(|message| message.content.contains("Available local skills"))
        .expect("skill catalog system message should exist");
    assert!(skill_message.content.contains("acpx"));
    assert!(
        skill_message
            .content
            .contains("agent-to-agent communication")
    );
}

#[tokio::test]
async fn feature_prompt_rebuilder_only_updates_live_feature_slots() {
    // 测试场景: build_messages 只负责生成初始化消息，不能直接改写 persisted history。
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let rebuilder = FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
        "",
    );
    let now = chrono::Utc::now();
    let mut thread_context = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_feature_history",
            "thread_feature_history",
        ),
        now,
    );
    thread_context.commit_test_turn(
        None,
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "persisted history",
            now,
        )],
        now,
        now,
    );

    let built_messages = rebuilder
        .build_messages(&thread_context, false)
        .await
        .expect("feature prompts should build");

    assert_eq!(thread_context.non_system_messages().len(), 1);
    assert_eq!(
        thread_context.non_system_messages()[0].content,
        "persisted history"
    );
    assert!(
        built_messages
            .iter()
            .any(|message| message.content.contains("OpenJarvis tool-use mode"))
    );
    assert!(
        !built_messages
            .iter()
            .any(|message| message.content.contains("Auto-compact 已开启"))
    );
    assert!(
        !thread_context
            .messages()
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
}

#[tokio::test]
async fn thread_initialization_keeps_feature_snapshot_stable_after_runtime_changes() {
    // 测试场景: feature prompt 一旦由 Thread 初始化落盘，后续线程状态变化或重新 attach runtime 都不能覆盖旧快照。
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo toolset for stable snapshot"),
            vec![Arc::new(DemoFeatureTool)],
        )
        .await
        .expect("demo toolset should register");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let now = chrono::Utc::now();
    let mut thread_context = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_feature_snapshot",
            "thread_feature_snapshot",
        ),
        now,
    );
    thread_context.attach_runtime(build_runtime_attachment(
        Arc::clone(&registry),
        "worker system prompt",
    ));

    let initialized = thread_context
        .ensure_initialized()
        .await
        .expect("thread initialization should succeed");
    let initial_catalog = thread_context
        .system_messages()
        .into_iter()
        .find(|message| message.content.contains("Available toolsets"))
        .expect("toolset catalog prompt should exist")
        .content;

    assert!(initialized);
    assert!(initial_catalog.contains("Currently loaded toolsets for this thread: none"));

    assert!(thread_context.load_toolset("demo"));
    thread_context.attach_runtime(build_runtime_attachment(
        Arc::clone(&registry),
        "worker system prompt changed",
    ));
    let changed = thread_context
        .ensure_initialized()
        .await
        .expect("re-attached runtime should keep old snapshot");

    let stable_catalog = thread_context
        .system_messages()
        .into_iter()
        .find(|message| message.content.contains("Available toolsets"))
        .expect("toolset catalog prompt should still exist")
        .content;

    assert!(!changed);
    assert_eq!(stable_catalog, initial_catalog);
    assert!(
        !stable_catalog.contains("Currently loaded toolsets for this thread: demo"),
        "稳定初始化快照不能被运行时加载状态覆盖"
    );
}

#[tokio::test]
async fn auto_compactor_only_decides_budget_thresholds_without_injecting_messages() {
    // 测试场景: AutoCompactor 只负责预算阈值判断，不能再向请求消息里注入 transient system prompt。
    let compact_config = AppConfig::default().agent_config().compact_config().clone();
    let auto_compactor = AutoCompactor::new(compact_config.clone());
    let now = chrono::Utc::now();
    let thread_context = Thread::new(
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
            (ContextTokenKind::Chat, 160),
            (ContextTokenKind::VisibleTool, 16),
            (ContextTokenKind::ReservedOutput, 16),
        ]),
        256,
    );

    assert!(thread_context.system_messages().is_empty());
    assert!(auto_compactor.compact_tool_visible(&budget_report));
    assert!(auto_compactor.runtime_compaction_required(&budget_report));
    assert!(
        !thread_context
            .messages()
            .iter()
            .any(|message| message.content.contains("<context capacity"))
    );
}
