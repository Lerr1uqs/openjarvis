use openjarvis::{
    agent::{CompactToolProjection, ToolRegistry},
    compact::ContextBudgetReport,
    context::ContextTokenKind,
};
use std::collections::HashMap;

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
