use super::{browser::BrowserFixture, build_thread, call_tool, list_tools};
use anyhow::Result;
use async_trait::async_trait;
use openjarvis::agent::{
    ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
    ToolsetCatalogEntry, empty_tool_input_schema,
    tool::browser::register_browser_toolset_with_config,
};
use openjarvis::thread::{ThreadAgent, ThreadAgentKind};
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

#[tokio::test]
async fn browser_kind_hides_optional_toolset_controls_and_catalog() {
    // 测试场景: Browser kind 的 browser 能力属于默认绑定 truth，
    // 不视为可选 toolset，因此不会看到可选 catalog，也不能通过 unload_toolset 卸载。
    let fixture = BrowserFixture::new("openjarvis-browser-kind-toolset");
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    let mut thread_browser = build_thread("thread_browser_kind");
    thread_browser.replace_thread_agent(ThreadAgent::from_kind(ThreadAgentKind::Browser));
    thread_browser.replace_loaded_toolsets(vec!["browser".to_string()]);

    let visible_tools = list_tools(&registry, &thread_browser)
        .await
        .expect("browser kind tools should list");
    let visible_tool_names = visible_tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert!(visible_tool_names.contains(&"browser__navigate"));
    assert!(visible_tool_names.contains(&"browser__snapshot"));
    assert!(!visible_tool_names.contains(&"load_toolset"));
    assert!(!visible_tool_names.contains(&"unload_toolset"));
    assert!(!visible_tool_names.contains(&"demo__echo"));

    assert!(
        registry
            .catalog_prompt_for_context(&thread_browser)
            .await
            .is_none()
    );

    let error = call_tool(
        &registry,
        &mut thread_browser,
        ToolCallRequest {
            name: "unload_toolset".to_string(),
            arguments: json!({ "name": "browser" }),
        },
    )
    .await
    .expect_err("browser kind should not expose unload_toolset control");
    assert!(
        error
            .to_string()
            .contains("tool `unload_toolset` is not enabled")
    );
    assert_eq!(thread_browser.load_toolsets(), vec!["browser".to_string()]);
}

#[tokio::test]
async fn main_kind_filters_browser_from_optional_toolset_catalog() {
    // 测试场景: Main 线程仍可加载其他 optional toolset，
    // 但 browser 相关工作套件必须被 kind profile 排除，只能通过 subagent 使用。
    let fixture = BrowserFixture::new("openjarvis-main-browser-filter");
    let registry = ToolRegistry::new();
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo", "Demo thread-managed toolset"),
            vec![Arc::new(DemoToolsetEchoTool)],
        )
        .await
        .expect("demo toolset should register");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    let mut thread_main = build_thread("thread_main_browser_filter");

    let visible_tools = list_tools(&registry, &thread_main)
        .await
        .expect("main kind tools should list");
    let visible_tool_names = visible_tools
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert!(visible_tool_names.contains(&"load_toolset"));
    assert!(visible_tool_names.contains(&"unload_toolset"));
    assert!(!visible_tool_names.contains(&"browser__navigate"));
    assert!(!visible_tool_names.contains(&"demo__echo"));

    let catalog_prompt = registry
        .catalog_prompt_for_context(&thread_main)
        .await
        .expect("main kind should still expose optional catalog");
    assert!(catalog_prompt.contains("demo"));
    assert!(!catalog_prompt.contains("browser"));

    let browser_error = call_tool(
        &registry,
        &mut thread_main,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "browser" }),
        },
    )
    .await
    .expect_err("main kind should reject browser toolset loading");
    assert!(
        browser_error
            .to_string()
            .contains("toolset `browser` is not available for thread agent kind `main`")
    );

    let demo_result = call_tool(
        &registry,
        &mut thread_main,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "demo" }),
        },
    )
    .await
    .expect("main kind should still load non-browser toolsets");
    assert_eq!(demo_result.metadata["toolset"], "demo");
    assert_eq!(thread_main.load_toolsets(), vec!["demo".to_string()]);
}
