use super::BrowserFixture;
use openjarvis::agent::tool::browser::BrowserSessionManager;
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
