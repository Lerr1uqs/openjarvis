use super::BrowserFixture;
use openjarvis::agent::tool::browser::{
    BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSidecarService,
    BrowserSidecarServiceConfig, default_sidecar_script_path,
};

#[tokio::test]
async fn browser_sidecar_service_executes_mock_flow_and_writes_screenshot() {
    // 验证 mock sidecar 能完成 navigate -> snapshot -> screenshot -> close 闭环。
    let fixture = BrowserFixture::new("openjarvis-browser-service-mock");
    let screenshot_path = fixture.root().join("service-shot.png");
    let mut service = BrowserSidecarService::new(fixture.service_config(true));

    let navigate = service
        .navigate("https://example.com")
        .await
        .expect("navigate should succeed");
    let snapshot = service
        .snapshot(Some(1))
        .await
        .expect("snapshot should succeed");
    let screenshot = service
        .screenshot(&screenshot_path)
        .await
        .expect("screenshot should succeed");
    let close = service.close().await.expect("close should succeed");

    assert_eq!(navigate.title, "Example Domain");
    assert_eq!(snapshot.elements.len(), 1);
    assert_eq!(snapshot.total_candidate_count, 2);
    assert!(snapshot.truncated);
    assert_eq!(screenshot.path, screenshot_path.display().to_string());
    assert!(screenshot_path.exists());
    assert!(close.closed);
}

#[tokio::test]
async fn browser_sidecar_service_reports_mock_protocol_errors() {
    // 验证 sidecar 返回 missing_ref 时 service 会把错误向上传递。
    let fixture = BrowserFixture::new("openjarvis-browser-service-error");
    let mut service = BrowserSidecarService::new(fixture.service_config(true));

    let _ = service
        .navigate("https://example.com")
        .await
        .expect("navigate should succeed");
    let _ = service
        .snapshot(None)
        .await
        .expect("snapshot should succeed");
    let error = service
        .click_ref("missing-ref")
        .await
        .expect_err("unknown ref should fail");

    assert!(error.to_string().contains("missing_ref"));
}

#[tokio::test]
#[ignore]
async fn browser_sidecar_service_smoke_runs_against_real_node_sidecar() {
    // 验证真实 Node sidecar 至少能跑通最小烟测链路。
    let fixture = BrowserFixture::new("openjarvis-browser-service-smoke");
    let session_root = fixture.root().join("real-sidecar");
    let user_data_dir = session_root.join("user-data");
    std::fs::create_dir_all(&user_data_dir).expect("real smoke user data dir should exist");
    let screenshot_path = fixture.root().join("real-sidecar-shot.png");
    let mut service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
        BrowserProcessCommandSpec::node_sidecar(default_sidecar_script_path()),
        BrowserRuntimeOptions {
            headless: true,
            keep_artifacts: true,
            ..Default::default()
        },
        session_root,
        user_data_dir,
    ));

    let navigate = service
        .navigate("https://example.com")
        .await
        .expect("real sidecar navigate should succeed");
    let snapshot = service
        .snapshot(Some(120))
        .await
        .expect("real sidecar snapshot should succeed");
    let screenshot = service
        .screenshot(&screenshot_path)
        .await
        .expect("real sidecar screenshot should succeed");
    let close = service
        .close()
        .await
        .expect("real sidecar close should succeed");

    assert!(navigate.url.contains("example.com"));
    assert!(!snapshot.snapshot_text.is_empty());
    assert_eq!(screenshot.path, screenshot_path.display().to_string());
    assert!(screenshot_path.exists());
    assert!(close.closed);
}
