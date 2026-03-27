use super::BrowserFixture;
use openjarvis::agent::{
    ToolCallRequest, ToolRegistry, tool::browser::register_browser_toolset_with_config,
};
use serde_json::json;
use std::path::PathBuf;

#[tokio::test]
async fn browser_toolset_exposes_thread_loaded_tools_and_runs_mock_actions() {
    // 验证 browser toolset 加载后会暴露核心工具，并能跑通基础 mock 动作。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset");
    let registry = ToolRegistry::new();
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    let initial_tools = registry
        .list_for_thread("thread-browser")
        .await
        .expect("initial tools should list");
    assert!(
        !initial_tools
            .iter()
            .any(|definition| definition.name == "browser__navigate")
    );

    registry
        .call_for_thread(
            "thread-browser",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "browser" }),
            },
        )
        .await
        .expect("browser toolset should load");

    let loaded_tools = registry
        .list_for_thread("thread-browser")
        .await
        .expect("loaded tools should list");
    assert!(
        loaded_tools
            .iter()
            .any(|definition| definition.name == "browser__navigate")
    );
    assert!(
        loaded_tools
            .iter()
            .any(|definition| definition.name == "browser__snapshot")
    );
    assert!(
        loaded_tools
            .iter()
            .any(|definition| definition.name == "browser__click_match")
    );
    assert!(
        loaded_tools
            .iter()
            .any(|definition| definition.name == "browser__type_match")
    );

    let navigate = registry
        .call_for_thread(
            "thread-browser",
            ToolCallRequest {
                name: "browser__navigate".to_string(),
                arguments: json!({ "url": "https://example.com" }),
            },
        )
        .await
        .expect("navigate should succeed");
    let snapshot = registry
        .call_for_thread(
            "thread-browser",
            ToolCallRequest {
                name: "browser__snapshot".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("snapshot should succeed");
    let screenshot = registry
        .call_for_thread(
            "thread-browser",
            ToolCallRequest {
                name: "browser__screenshot".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("screenshot should succeed");

    assert!(navigate.content.contains("example.com"));
    assert!(snapshot.content.contains("[1]"));
    assert!(PathBuf::from(&screenshot.content).exists());
}

#[tokio::test]
async fn browser_toolset_match_tools_resolve_elements_without_stable_refs() {
    // 验证 match 工具可以根据元素特征重新定位目标，而不是依赖瞬时 ref。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-match");
    let registry = ToolRegistry::new();
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    registry
        .call_for_thread(
            "thread-browser-match",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "browser" }),
            },
        )
        .await
        .expect("browser toolset should load");

    let _ = registry
        .call_for_thread(
            "thread-browser-match",
            ToolCallRequest {
                name: "browser__navigate".to_string(),
                arguments: json!({ "url": "https://example.com" }),
            },
        )
        .await
        .expect("navigate should succeed");

    let click = registry
        .call_for_thread(
            "thread-browser-match",
            ToolCallRequest {
                name: "browser__click_match".to_string(),
                arguments: json!({
                    "role": "link",
                    "href_contains": "example.com/more",
                }),
            },
        )
        .await
        .expect("click_match should succeed");
    let typed = registry
        .call_for_thread(
            "thread-browser-match",
            ToolCallRequest {
                name: "browser__type_match".to_string(),
                arguments: json!({
                    "role": "textbox",
                    "placeholder_contains": "search",
                    "text": "hello world",
                    "submit": true,
                }),
            },
        )
        .await
        .expect("type_match should succeed");

    assert!(click.content.contains("matched ref"));
    assert_eq!(click.metadata["matched_element"]["ref"], "1");
    assert_eq!(typed.metadata["matched_element"]["ref"], "2");
    assert_eq!(typed.metadata["submitted"], true);
}

#[tokio::test]
async fn browser_toolset_unload_closes_session_and_hides_tools() {
    // 验证卸载 toolset 后会同步关闭 session 并移除线程可见工具。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-unload");
    let registry = ToolRegistry::new();
    register_browser_toolset_with_config(&registry, fixture.manager_config(false))
        .await
        .expect("browser toolset should register");

    registry
        .call_for_thread(
            "thread-browser-unload",
            ToolCallRequest {
                name: "load_toolset".to_string(),
                arguments: json!({ "name": "browser" }),
            },
        )
        .await
        .expect("browser toolset should load");
    let _ = registry
        .call_for_thread(
            "thread-browser-unload",
            ToolCallRequest {
                name: "browser__navigate".to_string(),
                arguments: json!({ "url": "https://example.com" }),
            },
        )
        .await
        .expect("navigate should succeed");
    let screenshot = registry
        .call_for_thread(
            "thread-browser-unload",
            ToolCallRequest {
                name: "browser__screenshot".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("screenshot should succeed");

    assert!(PathBuf::from(&screenshot.content).exists());

    registry
        .call_for_thread(
            "thread-browser-unload",
            ToolCallRequest {
                name: "unload_toolset".to_string(),
                arguments: json!({ "name": "browser" }),
            },
        )
        .await
        .expect("browser toolset should unload");

    assert!(!PathBuf::from(&screenshot.content).exists());
    let error = registry
        .call_for_thread(
            "thread-browser-unload",
            ToolCallRequest {
                name: "browser__snapshot".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect_err("unloaded browser tool should not be callable");
    assert!(error.to_string().contains("not registered for thread"));
}
