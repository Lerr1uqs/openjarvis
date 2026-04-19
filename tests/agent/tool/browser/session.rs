use super::BrowserFixture;
use openjarvis::agent::tool::browser::{
    BrowserDiagnosticsQuery, BrowserOpenRequest, BrowserRequestDiagnosticsQuery,
    BrowserRuntimeOptions, BrowserSessionManager, BrowserSessionMode,
};
use std::path::PathBuf;

#[tokio::test]
async fn browser_session_manager_isolates_threads_and_removes_temp_artifacts() {
    let fixture = BrowserFixture::new("openjarvis-browser-session-remove");
    let manager = BrowserSessionManager::new(fixture.manager_config(false));

    let _ = manager
        .navigate("thread-a", "https://example.com")
        .await
        .expect("thread a navigate should succeed");
    let _ = manager
        .navigate("thread-b", "https://localhost")
        .await
        .expect("thread b navigate should succeed");
    let screenshot = manager
        .screenshot("thread-a", None)
        .await
        .expect("thread a screenshot should succeed");

    assert!(manager.has_session("thread-a").await);
    assert!(manager.has_session("thread-b").await);
    assert!(PathBuf::from(&screenshot.path).exists());

    let close = manager
        .close("thread-a")
        .await
        .expect("close should succeed");

    assert!(close.had_session);
    assert!(!close.kept_artifacts);
    assert!(!manager.has_session("thread-a").await);
    assert!(manager.has_session("thread-b").await);
    assert!(!PathBuf::from(&screenshot.path).exists());
}

#[tokio::test]
async fn browser_session_manager_keeps_artifacts_when_configured() {
    let fixture = BrowserFixture::new("openjarvis-browser-session-keep");
    let manager = BrowserSessionManager::new(fixture.manager_config(true));

    let _ = manager
        .navigate("thread-keep", "https://example.com")
        .await
        .expect("navigate should succeed");
    let screenshot = manager
        .screenshot("thread-keep", None)
        .await
        .expect("screenshot should succeed");
    let close = manager
        .close("thread-keep")
        .await
        .expect("close should succeed");

    assert!(close.had_session);
    assert!(close.kept_artifacts);
    assert!(close.artifacts.is_some());
    assert!(PathBuf::from(&screenshot.path).exists());
}

#[tokio::test]
async fn browser_session_manager_replaces_launch_session_with_attach_session() {
    // 测试场景: 同一线程再次 open 时必须替换已有 session 来源，而不是并存。
    let fixture = BrowserFixture::new("openjarvis-browser-session-replace");
    let manager = BrowserSessionManager::new(fixture.manager_config(true));

    let first_open = manager
        .open("thread-replace", BrowserOpenRequest::launch())
        .await
        .expect("launch open should succeed");
    let second_open = manager
        .open(
            "thread-replace",
            BrowserOpenRequest::attach("http://127.0.0.1:9222"),
        )
        .await
        .expect("attach open should succeed");
    let close = manager
        .close("thread-replace")
        .await
        .expect("close should succeed");

    assert_eq!(first_open.mode, BrowserSessionMode::Launch);
    assert_eq!(second_open.mode, BrowserSessionMode::Attach);
    assert_eq!(close.session_mode, Some(BrowserSessionMode::Attach));
}

#[tokio::test]
async fn browser_session_manager_exports_cookies_and_reports_auto_export_on_close() {
    // 测试场景: 手动导出和 close 自动导出都要在 session manager 层可见。
    let fixture = BrowserFixture::new("openjarvis-browser-session-export");
    let mut config = fixture.manager_config(true);
    let auto_export_path = fixture.root().join("state/auto-cookies.json");
    config.runtime = BrowserRuntimeOptions {
        keep_artifacts: true,
        cookies_state_file: Some(auto_export_path.clone()),
        save_cookies_on_close: true,
        ..config.runtime.clone()
    };
    let manager = BrowserSessionManager::new(config);
    let manual_export_path = fixture.root().join("manual/exported-cookies.json");

    let _ = manager
        .navigate("thread-export", "https://example.com")
        .await
        .expect("navigate should succeed");
    let export = manager
        .export_cookies("thread-export", &manual_export_path)
        .await
        .expect("manual export should succeed");
    let close = manager
        .close("thread-export")
        .await
        .expect("close should succeed");

    assert_eq!(export.mode, BrowserSessionMode::Launch);
    assert_eq!(export.path, manual_export_path.display().to_string());
    assert_eq!(export.cookie_count, 0);
    assert!(manual_export_path.exists());
    assert_eq!(
        close.auto_exported_path.as_deref(),
        Some(auto_export_path.to_string_lossy().as_ref())
    );
    assert_eq!(close.exported_cookie_count, Some(0));
    assert!(auto_export_path.exists());
}

#[tokio::test]
async fn browser_session_manager_returns_recent_diagnostics_without_reopening_session() {
    // 测试场景: 诊断查询应复用当前 session，并在 close 后返回空结果而不是重建新会话。
    let fixture = BrowserFixture::new("openjarvis-browser-session-diagnostics");
    let manager = BrowserSessionManager::new(fixture.manager_config(true));

    let _ = manager
        .navigate("thread-diagnostics", "https://example.com/fail")
        .await
        .expect("navigate should succeed");
    let console = manager
        .console("thread-diagnostics", BrowserDiagnosticsQuery::new(Some(5)))
        .await
        .expect("console query should succeed");
    let failed_requests = manager
        .requests(
            "thread-diagnostics",
            BrowserRequestDiagnosticsQuery::new(Some(5), true),
        )
        .await
        .expect("failed requests query should succeed");

    assert!(!console.entries.is_empty());
    assert!(!failed_requests.entries.is_empty());

    let _ = manager
        .close("thread-diagnostics")
        .await
        .expect("close should succeed");
    let after_close = manager
        .errors("thread-diagnostics", BrowserDiagnosticsQuery::new(Some(5)))
        .await
        .expect("errors query after close should succeed");

    assert!(after_close.entries.is_empty());
}
