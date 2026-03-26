use anyhow::Result;
use async_trait::async_trait;
use openjarvis::agent::{
    ThreadToolRuntimeManager, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    ToolRegistry, ToolsetCatalogEntry, empty_tool_input_schema,
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
async fn thread_tool_runtime_manager_tracks_loaded_toolsets_per_thread() {
    let manager = ThreadToolRuntimeManager::new();

    assert!(manager.load_toolset("thread_a", "browser").await);
    assert!(!manager.load_toolset("thread_a", "browser").await);
    assert!(manager.load_toolset("thread_b", "browser").await);
    assert!(manager.unload_toolset("thread_a", "browser").await);
    assert!(!manager.unload_toolset("thread_a", "browser").await);

    assert!(manager.loaded_toolsets("thread_a").await.is_empty());
    assert_eq!(
        manager.loaded_toolsets("thread_b").await,
        vec!["browser".to_string()]
    );
}

#[tokio::test]
async fn tool_registry_exposes_catalog_and_thread_scoped_load_unload() {
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");

    let toolsets = registry.list_toolsets().await;
    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].name, "demo");

    let initial_tools = registry
        .list_for_thread("thread_demo")
        .await
        .expect("thread-scoped tool listing should succeed");
    let initial_names = initial_tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(initial_names, vec!["load_toolset", "unload_toolset"]);

    let catalog_prompt = registry
        .catalog_prompt("thread_demo")
        .await
        .expect("catalog prompt should be available");
    assert!(catalog_prompt.contains("demo"));
    assert!(catalog_prompt.contains("Currently loaded toolsets for this thread: none"));

    let load_result = registry
        .call_for_thread(
            "thread_demo",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "demo" }),
            },
        )
        .await
        .expect("demo toolset should load");
    assert_eq!(load_result.metadata["event_kind"], "load_toolset");
    assert_eq!(load_result.metadata["toolset"], "demo");

    let loaded_tools = registry
        .list_for_thread("thread_demo")
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

    let call_result = registry
        .call_for_thread(
            "thread_demo",
            ToolCallRequest {
                name: "demo__echo".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("thread-scoped toolset tool should execute");
    assert_eq!(call_result.content, "demo-toolset-echo");

    let isolated_tools = registry
        .list_for_thread("thread_other")
        .await
        .expect("other thread should keep isolated tool visibility");
    assert_eq!(
        isolated_tools
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["load_toolset", "unload_toolset"]
    );

    let unload_result = registry
        .call_for_thread(
            "thread_demo",
            ToolCallRequest {
                name: "unload_toolset".to_string(),
                arguments: json!({ "name": "demo" }),
            },
        )
        .await
        .expect("demo toolset should unload");
    assert_eq!(unload_result.metadata["event_kind"], "unload_toolset");
    assert_eq!(
        registry
            .loaded_toolsets_for_thread("thread_demo")
            .await
            .len(),
        0
    );
}

#[tokio::test]
async fn tool_registry_rejects_toolset_tool_calls_after_unload() {
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");

    registry
        .call_for_thread(
            "thread_demo_after_unload",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "demo" }),
            },
        )
        .await
        .expect("demo toolset should load");

    let call_result = registry
        .call_for_thread(
            "thread_demo_after_unload",
            ToolCallRequest {
                name: "demo__echo".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("loaded toolset tool should execute");
    assert_eq!(call_result.content, "demo-toolset-echo");

    registry
        .call_for_thread(
            "thread_demo_after_unload",
            ToolCallRequest {
                name: "unload_toolset".to_string(),
                arguments: json!({ "name": "demo" }),
            },
        )
        .await
        .expect("demo toolset should unload");

    let error = registry
        .call_for_thread(
            "thread_demo_after_unload",
            ToolCallRequest {
                name: "demo__echo".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect_err("unloaded toolset tool should no longer be callable");
    assert!(error.to_string().contains("not registered for thread"));
}
