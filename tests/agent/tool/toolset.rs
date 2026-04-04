use super::{build_thread, call_tool, list_tools};
use anyhow::Result;
use async_trait::async_trait;
use openjarvis::agent::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
    ToolsetCatalogEntry, empty_tool_input_schema,
};
use serde_json::json;
use std::sync::Arc;

struct DemoToolsetEchoTool;

#[async_trait]
impl ToolHandler for DemoToolsetEchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo__echo".to_string(),
            description: "Echo from the demo toolset".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "demo-toolset-echo".to_string(),
            metadata: json!({ "toolset": "demo" }),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn tool_registry_exposes_catalog_and_thread_scoped_load_unload() {
    // 测试场景: toolset 可见性和加载状态只由 Thread 决定，registry 不再维护第二份线程真相。
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");
    let mut thread_demo = build_thread("thread_demo");
    let thread_other = build_thread("thread_other");

    let toolsets = registry.list_toolsets().await;
    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].name, "demo");

    let initial_tools = list_tools(&registry, &thread_demo)
        .await
        .expect("thread-scoped tool listing should succeed");
    let initial_names = initial_tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(initial_names, vec!["load_toolset", "unload_toolset"]);

    let catalog_prompt = registry
        .catalog_prompt_for_context(&thread_demo)
        .await
        .expect("catalog prompt should be available");
    assert!(catalog_prompt.contains("demo"));
    assert!(catalog_prompt.contains("Currently loaded toolsets for this thread: none"));

    let load_result = call_tool(
        &registry,
        &mut thread_demo,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("demo toolset should load");
    assert_eq!(load_result.metadata["event_kind"], "load_toolset");
    assert_eq!(load_result.metadata["toolset"], "demo");
    assert_eq!(thread_demo.load_toolsets(), vec!["demo".to_string()]);

    let loaded_tools = list_tools(&registry, &thread_demo)
        .await
        .expect("loaded thread should expose toolset tools");
    let loaded_names = loaded_tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        loaded_names,
        vec!["demo__echo", "load_toolset", "unload_toolset"]
    );

    let call_result = call_tool(
        &registry,
        &mut thread_demo,
        ToolCallRequest {
            name: "demo__echo".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("thread-scoped toolset tool should execute");
    assert_eq!(call_result.content, "demo-toolset-echo");

    let isolated_tools = list_tools(&registry, &thread_other)
        .await
        .expect("other thread should keep isolated tool visibility");
    assert_eq!(
        isolated_tools
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["load_toolset", "unload_toolset"]
    );

    let unload_result = call_tool(
        &registry,
        &mut thread_demo,
        ToolCallRequest {
            name: "unload_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("demo toolset should unload");
    assert_eq!(unload_result.metadata["event_kind"], "unload_toolset");
    assert!(thread_demo.load_toolsets().is_empty());
}

#[tokio::test]
async fn tool_registry_rejects_toolset_tool_calls_after_unload() {
    // 测试场景: 卸载后 thread snapshot 不再暴露对应 toolset 工具。
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");
    let mut thread_context = build_thread("thread_demo_after_unload");

    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("demo toolset should load");

    let call_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "demo__echo".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("loaded toolset tool should execute");
    assert_eq!(call_result.content, "demo-toolset-echo");

    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "unload_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("demo toolset should unload");

    let error = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "demo__echo".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect_err("unloaded toolset tool should no longer be callable");
    assert!(error.to_string().contains("not registered for thread"));
}
