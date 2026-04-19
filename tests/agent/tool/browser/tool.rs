use super::BrowserFixture;
use crate::agent::tool::{build_thread, call_tool, list_tools};
use openjarvis::agent::{
    ToolCallRequest, ToolRegistry,
    tool::browser::{BrowserRuntimeOptions, register_browser_toolset_with_config},
};
use openjarvis::thread::{Thread, ThreadAgent, ThreadAgentKind};
use serde_json::json;
use std::path::PathBuf;

fn build_browser_thread(thread_id: &str) -> Thread {
    let mut thread_context = build_thread(thread_id);
    thread_context.replace_thread_agent(ThreadAgent::from_kind(ThreadAgentKind::Browser));
    thread_context.replace_loaded_toolsets(vec!["browser".to_string()]);
    thread_context
}

#[tokio::test]
async fn browser_thread_exposes_bound_browser_tools_and_runs_mock_actions() {
    // 验证 Browser 子线程天然拥有 browser 绑定工具，并能跑通基础 mock 动作。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    let initial_tools = list_tools(&registry, &thread_context)
        .await
        .expect("initial tools should list");
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__open")
    );
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__navigate")
    );
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__snapshot")
    );
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__console")
    );
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__errors")
    );
    assert!(
        initial_tools
            .iter()
            .any(|definition| definition.name == "browser__requests")
    );
    assert!(
        !initial_tools
            .iter()
            .any(|definition| definition.name == "browser__export_cookies")
    );
    assert!(
        !initial_tools
            .iter()
            .any(|definition| definition.name == "browser__load_cookies")
    );

    let navigate = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__navigate".to_string(),
            arguments: json!({ "url": "https://example.com" }),
        },
    )
    .await
    .expect("navigate should succeed");
    let snapshot = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__snapshot".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("snapshot should succeed");
    let screenshot = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__screenshot".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("screenshot should succeed");

    // Bug regression: navigate 结果必须直接返回轻量 snapshot，避免模型在没有页面观察结果时重复 navigate。
    assert!(navigate.content.contains("example.com"));
    assert!(navigate.content.contains("Current page snapshot:"));
    assert!(navigate.content.contains("[1]"));
    assert!(snapshot.content.contains("[1]"));
    assert!(PathBuf::from(&screenshot.content).exists());
}

#[tokio::test]
async fn browser_thread_match_tools_resolve_elements_without_stable_refs() {
    // 验证 Browser 子线程内的 match 工具可以根据元素特征重新定位目标，而不是依赖瞬时 ref。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-match");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser-match");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");
    let _ = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__navigate".to_string(),
            arguments: json!({ "url": "https://example.com" }),
        },
    )
    .await
    .expect("navigate should succeed");

    let click = call_tool(
        &registry,
        &mut thread_context,
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
    let typed = call_tool(
        &registry,
        &mut thread_context,
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
async fn browser_thread_diagnostic_tools_query_recent_records() {
    // 测试场景: Browser 子线程诊断工具应返回结构化记录，并支持 failed_only 过滤。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-diagnostics");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser-diagnostics");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");
    let _ = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__navigate".to_string(),
            arguments: json!({ "url": "https://example.com/error" }),
        },
    )
    .await
    .expect("navigate should succeed");

    let console = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__console".to_string(),
            arguments: json!({ "limit": 1 }),
        },
    )
    .await
    .expect("console query should succeed");
    let errors = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__errors".to_string(),
            arguments: json!({ "limit": 5 }),
        },
    )
    .await
    .expect("errors query should succeed");
    let failed_requests = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__requests".to_string(),
            arguments: json!({ "failed_only": true, "limit": 5 }),
        },
    )
    .await
    .expect("requests query should succeed");

    assert_eq!(console.metadata["entry_count"], 1);
    assert!(console.content.contains("Navigated"));
    assert!(errors.content.contains("page_error"));
    assert_eq!(failed_requests.metadata["entries"][0]["result"], "failed");
}

#[tokio::test]
async fn browser_thread_open_supports_attach_mode_and_close_reports_mode() {
    // 测试场景: Browser 子线程的 browser__open 要支持 attach，并在 close metadata 中保留 session mode。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-open-attach");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser-open-attach");
    register_browser_toolset_with_config(&registry, fixture.manager_config(true))
        .await
        .expect("browser toolset should register");

    let open = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__open".to_string(),
            arguments: json!({
                "mode": "attach",
                "cdp_endpoint": "http://127.0.0.1:9222",
            }),
        },
    )
    .await
    .expect("browser open should succeed");
    let close = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__close".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("browser close should succeed");

    assert_eq!(open.metadata["mode"], "attach");
    assert_eq!(close.metadata["mode"], "attach");
}

#[tokio::test]
async fn browser_close_tool_closes_session_and_cleans_artifacts() {
    // 验证 Browser 子线程通过 browser__close 关闭 session 后会回收产物，
    // browser 能力仍属于绑定能力，不再通过主线程 unload_toolset 暴露。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-close");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser-close");
    register_browser_toolset_with_config(&registry, fixture.manager_config(false))
        .await
        .expect("browser toolset should register");
    let _ = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__navigate".to_string(),
            arguments: json!({ "url": "https://example.com" }),
        },
    )
    .await
    .expect("navigate should succeed");
    let screenshot = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__screenshot".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("screenshot should succeed");

    assert!(PathBuf::from(&screenshot.content).exists());

    let close_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__close".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("browser close should succeed");
    assert_eq!(close_result.content, "Browser session closed.");
    assert_eq!(close_result.metadata["toolset"], "browser");
    assert_eq!(close_result.metadata["had_session"], true);

    assert!(!PathBuf::from(&screenshot.content).exists());
    let second_close = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__close".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("browser close without session should still succeed");
    assert_eq!(
        second_close.content,
        "No browser session was active for the current thread."
    );
    assert_eq!(second_close.metadata["had_session"], false);
}

#[tokio::test]
async fn browser_close_tool_reports_auto_exported_cookie_file_when_enabled() {
    // 测试场景: 开启 close 自动导出后，Browser 子线程的 browser__close 结果必须带回导出摘要。
    let fixture = BrowserFixture::new("openjarvis-browser-toolset-close-export");
    let registry = ToolRegistry::new();
    let mut thread_context = build_browser_thread("thread-browser-close-export");
    let mut manager_config = fixture.manager_config(true);
    let cookies_state_file = fixture.root().join("state/browser-cookies.json");
    manager_config.runtime = BrowserRuntimeOptions {
        keep_artifacts: true,
        cookies_state_file: Some(cookies_state_file.clone()),
        save_cookies_on_close: true,
        ..manager_config.runtime.clone()
    };
    register_browser_toolset_with_config(&registry, manager_config)
        .await
        .expect("browser toolset should register");
    let _ = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__navigate".to_string(),
            arguments: json!({ "url": "https://example.com" }),
        },
    )
    .await
    .expect("navigate should succeed");

    let close = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "browser__close".to_string(),
            arguments: json!({}),
        },
    )
    .await
    .expect("browser close should succeed");

    assert_eq!(
        close.metadata["auto_exported_path"],
        cookies_state_file.display().to_string()
    );
    assert_eq!(close.metadata["exported_cookie_count"], 0);
    assert!(cookies_state_file.exists());
}
