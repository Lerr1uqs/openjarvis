use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        CompactToolProjection, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
        ToolRegistry, ToolsetCatalogEntry, empty_tool_input_schema,
    },
    compact::ContextBudgetReport,
    context::ContextTokenKind,
    thread::{ThreadContext, ThreadContextLocator},
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

struct DemoRegistryTool;

#[async_trait]
impl ToolHandler for DemoRegistryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo from the demo registry toolset".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "registry-demo".to_string(),
            metadata: json!({ "toolset": "demo" }),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn builtin_tools_can_be_registered_together() {
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let definitions = registry.list().await;
    let mut names = definitions
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    names.sort();

    assert_eq!(names, vec!["bash", "edit", "read", "write"]);
}

#[tokio::test]
async fn compact_tool_visibility_is_projected_per_thread() {
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let without_compact = registry
        .list_for_thread("thread_registry_compact")
        .await
        .expect("tool listing should succeed");
    assert!(!without_compact.iter().any(|tool| tool.name == "compact"));

    // 测试场景: 只有在 auto-compact projection 可见时，compact tool 才对当前线程暴露。
    registry
        .set_compact_tool_projection(
            "thread_registry_compact",
            Some(CompactToolProjection {
                auto_compact: true,
                visible: true,
                budget_report: ContextBudgetReport::new(
                    HashMap::from([
                        (ContextTokenKind::System, 10),
                        (ContextTokenKind::Chat, 80),
                        (ContextTokenKind::VisibleTool, 20),
                        (ContextTokenKind::ReservedOutput, 32),
                    ]),
                    200,
                ),
            }),
        )
        .await;
    let with_compact = registry
        .list_for_thread("thread_registry_compact")
        .await
        .expect("tool listing should succeed after projection");

    assert!(with_compact.iter().any(|tool| tool.name == "compact"));
}

#[tokio::test]
async fn deprecated_thread_entrypoints_forward_to_thread_context_runtime() {
    // 测试场景: deprecated thread-id 入口仍要转发到 ThreadContext 路径，且不能把 toolset 状态泄漏到其他线程。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo registry toolset"),
            vec![Arc::new(DemoRegistryTool)],
        )
        .await
        .expect("demo toolset should register");

    #[allow(deprecated)]
    let load_result = registry
        .call_for_thread(
            "thread_compat",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "demo" }),
            },
        )
        .await
        .expect("legacy load_toolset entrypoint should succeed");
    #[allow(deprecated)]
    let visible_for_loaded = registry
        .list_for_thread("thread_compat")
        .await
        .expect("loaded legacy thread should expose demo tool");
    #[allow(deprecated)]
    let visible_for_other = registry
        .list_for_thread("thread_other")
        .await
        .expect("other thread should keep isolated visibility");
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::for_internal_thread("thread_compat"),
        Utc::now(),
    );
    registry
        .merge_legacy_thread_state(&mut thread_context)
        .await;

    assert_eq!(load_result.metadata["loaded_toolsets"], json!(["demo"]));
    assert!(
        visible_for_loaded
            .iter()
            .any(|tool| tool.name == "demo__echo")
    );
    assert!(
        !visible_for_other
            .iter()
            .any(|tool| tool.name == "demo__echo")
    );
    assert_eq!(thread_context.load_toolsets(), vec!["demo".to_string()]);
}
