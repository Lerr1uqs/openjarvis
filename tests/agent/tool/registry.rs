use super::{build_thread, call_tool, list_tools};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
        ToolsetCatalogEntry, empty_tool_input_schema,
    },
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
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

    assert_eq!(
        names,
        vec![
            "bash",
            "edit",
            "exec_command",
            "list_unread_command_tasks",
            "read",
            "write",
            "write_stdin",
        ]
    );
}

#[tokio::test]
async fn compact_tool_visibility_is_controlled_by_request_state() {
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let thread_context = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_registry_compact",
            "thread_registry_compact",
        ),
        Utc::now(),
    );
    let without_compact = registry
        .list_for_context_with_compact(&thread_context, false)
        .await
        .expect("tool listing should succeed");
    assert!(!without_compact.iter().any(|tool| tool.name == "compact"));

    // 测试场景: 只有在当前 request state 显式要求暴露时，compact tool 才对模型可见。
    let with_compact = registry
        .list_for_context_with_compact(&thread_context, true)
        .await
        .expect("tool listing should succeed after request expose");

    assert!(with_compact.iter().any(|tool| tool.name == "compact"));
}

#[tokio::test]
async fn thread_state_is_loaded_from_thread_snapshot_only() {
    // 测试场景: registry 只消费 Thread 自身的 loaded_toolsets，不再恢复 thread-id keyed legacy cache。
    let registry = ToolRegistry::with_skill_roots(Vec::new());
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo registry toolset"),
            vec![Arc::new(DemoRegistryTool)],
        )
        .await
        .expect("demo toolset should register");
    let mut loaded_thread = build_thread("thread_loaded");
    let other_thread = build_thread("thread_other");

    let load_result = call_tool(
        &registry,
        &mut loaded_thread,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("load_toolset should succeed");
    let visible_for_loaded = list_tools(&registry, &loaded_thread)
        .await
        .expect("loaded thread should expose demo tool");
    let visible_for_other = list_tools(&registry, &other_thread)
        .await
        .expect("other thread should keep isolated visibility");

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
    assert_eq!(loaded_thread.load_toolsets(), vec!["demo".to_string()]);
}
