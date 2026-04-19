use super::BrowserFixture;
use openjarvis::agent::tool::browser::{
    BrowserDiagnosticsQuery, BrowserErrorKind, BrowserOpenRequest, BrowserProcessCommandSpec,
    BrowserRequestDiagnosticsQuery, BrowserRequestResultKind, BrowserRuntimeOptions,
    BrowserSessionMode, BrowserSidecarService, BrowserSidecarServiceConfig,
    default_sidecar_script_path,
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
async fn browser_sidecar_service_queries_recent_diagnostics_and_filters_failed_requests() {
    // 测试场景: service 应能查询 console/errors/requests，并支持 failed_only 过滤。
    let fixture = BrowserFixture::new("openjarvis-browser-service-diagnostics");
    let mut service = BrowserSidecarService::new(fixture.service_config(true));

    let _ = service
        .navigate("https://example.com/error")
        .await
        .expect("navigate should succeed");

    let console = service
        .console(BrowserDiagnosticsQuery::new(Some(1)))
        .await
        .expect("console query should succeed");
    let errors = service
        .errors(BrowserDiagnosticsQuery::new(Some(5)))
        .await
        .expect("errors query should succeed");
    let failed_requests = service
        .requests(BrowserRequestDiagnosticsQuery::new(Some(5), true))
        .await
        .expect("failed requests query should succeed");

    assert_eq!(console.entries.len(), 1);
    assert!(console.entries[0].text.contains("Navigated"));
    assert!(
        errors
            .entries
            .iter()
            .any(|entry| entry.kind == BrowserErrorKind::PageError)
    );
    assert!(
        errors
            .entries
            .iter()
            .any(|entry| entry.kind == BrowserErrorKind::RequestFailed)
    );
    assert!(!failed_requests.entries.is_empty());
    assert!(
        failed_requests
            .entries
            .iter()
            .all(|entry| entry.result == BrowserRequestResultKind::Failed)
    );
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
async fn browser_sidecar_service_keeps_diagnostic_artifacts_when_enabled() {
    // 测试场景: keep_artifacts 打开时，diagnostic 文件应在 session 目录中生成并保留。
    let fixture = BrowserFixture::new("openjarvis-browser-service-diagnostic-artifacts");
    let session_root = fixture.root().join("diagnostic-artifacts-session");
    let user_data_dir = session_root.join("user-data");
    std::fs::create_dir_all(&user_data_dir).expect("user data dir should exist");
    let mut service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
        super::mock_process_spec(),
        BrowserRuntimeOptions {
            headless: true,
            keep_artifacts: true,
            ..Default::default()
        },
        session_root.clone(),
        user_data_dir,
    ));

    let _ = service
        .navigate("https://example.com/fail")
        .await
        .expect("navigate should succeed");
    let _ = service.close().await.expect("close should succeed");

    let console_lines = std::fs::read_to_string(session_root.join("console.jsonl"))
        .expect("console artifact should exist");
    let error_lines = std::fs::read_to_string(session_root.join("errors.jsonl"))
        .expect("errors artifact should exist");
    let request_lines = std::fs::read_to_string(session_root.join("requests.jsonl"))
        .expect("requests artifact should exist");

    assert!(console_lines.lines().count() >= 1);
    assert!(error_lines.lines().count() >= 1);
    assert!(request_lines.lines().count() >= 2);
}

#[tokio::test]
async fn browser_sidecar_service_open_supports_attach_mode_and_explicit_cookie_export() {
    // 验证显式 open 可以进入 attach 模式，并允许手动导出 cookies 文件。
    let fixture = BrowserFixture::new("openjarvis-browser-service-open-attach");
    let export_path = fixture.root().join("cookies/export.json");
    let mut service = BrowserSidecarService::new(fixture.service_config(true));

    let open = service
        .open(BrowserOpenRequest::attach("http://127.0.0.1:9222"))
        .await
        .expect("attach open should succeed");
    let exported = service
        .export_cookies(&export_path)
        .await
        .expect("export_cookies should succeed");
    let close = service.close().await.expect("close should succeed");

    assert_eq!(open.mode, BrowserSessionMode::Attach);
    assert_eq!(exported.mode, BrowserSessionMode::Attach);
    assert_eq!(exported.path, export_path.display().to_string());
    assert_eq!(exported.cookie_count, 0);
    assert!(export_path.exists());
    assert_eq!(close.mode, Some(BrowserSessionMode::Attach));
}

#[tokio::test]
async fn browser_sidecar_service_close_reports_auto_export_summary() {
    // 测试场景: launch 会话开启 close 自动导出时，close 结果必须带回导出摘要。
    let fixture = BrowserFixture::new("openjarvis-browser-service-close-export");
    let session_root = fixture.root().join("auto-export-session");
    let user_data_dir = session_root.join("user-data");
    std::fs::create_dir_all(&user_data_dir).expect("user data dir should exist");
    let cookies_state_file = fixture.root().join("state/browser-cookies.json");
    let mut service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
        super::mock_process_spec(),
        BrowserRuntimeOptions {
            headless: true,
            keep_artifacts: true,
            cookies_state_file: Some(cookies_state_file.clone()),
            save_cookies_on_close: true,
            ..Default::default()
        },
        session_root,
        user_data_dir,
    ));

    let open = service
        .open(BrowserOpenRequest::launch())
        .await
        .expect("launch open should succeed");
    let close = service.close().await.expect("close should succeed");

    assert_eq!(open.mode, BrowserSessionMode::Launch);
    assert_eq!(close.mode, Some(BrowserSessionMode::Launch));
    assert_eq!(
        close.exported_cookies_path.as_deref(),
        Some(cookies_state_file.to_string_lossy().as_ref())
    );
    assert_eq!(close.exported_cookie_count, Some(0));
    assert!(cookies_state_file.exists());
}

#[tokio::test]
async fn browser_sidecar_service_launch_auto_loads_cookies_from_existing_state_file() {
    // 测试场景: launch 会话开启自动注入后，应在 open 结果里报告已加载的 cookies 数量。
    let fixture = BrowserFixture::new("openjarvis-browser-service-load-cookies");
    let session_root = fixture.root().join("load-cookies-session");
    let user_data_dir = session_root.join("user-data");
    std::fs::create_dir_all(&user_data_dir).expect("user data dir should exist");
    let cookies_state_file = fixture.root().join("state/browser-cookies.json");
    std::fs::create_dir_all(
        cookies_state_file
            .parent()
            .expect("cookies state parent should exist"),
    )
    .expect("cookies state dir should exist");
    std::fs::write(
        &cookies_state_file,
        serde_json::json!({
            "version": 1,
            "cookies": [{
                "name": "session",
                "value": "token",
                "domain": "example.com",
                "path": "/",
                "expires": -1,
                "httpOnly": true,
                "secure": true,
                "sameSite": "Lax",
            }],
        })
        .to_string(),
    )
    .expect("cookies state file should be written");
    let mut service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
        super::mock_process_spec(),
        BrowserRuntimeOptions {
            headless: true,
            keep_artifacts: true,
            cookies_state_file: Some(cookies_state_file),
            load_cookies_on_open: true,
            ..Default::default()
        },
        session_root,
        user_data_dir,
    ));

    let open = service
        .open(BrowserOpenRequest::launch())
        .await
        .expect("launch open should succeed");

    assert_eq!(open.mode, BrowserSessionMode::Launch);
    assert_eq!(open.cookies_loaded, 1);
}

#[tokio::test]
async fn browser_sidecar_service_missing_cookie_state_file_does_not_block_launch() {
    // 测试场景: 自动注入开启但状态文件缺失时，首次 launch 仍应成功建立空基线会话。
    let fixture = BrowserFixture::new("openjarvis-browser-service-missing-cookies");
    let session_root = fixture.root().join("missing-cookies-session");
    let user_data_dir = session_root.join("user-data");
    std::fs::create_dir_all(&user_data_dir).expect("user data dir should exist");
    let cookies_state_file = fixture.root().join("state/browser-cookies.json");
    let mut service = BrowserSidecarService::new(BrowserSidecarServiceConfig::new(
        super::mock_process_spec(),
        BrowserRuntimeOptions {
            headless: true,
            keep_artifacts: true,
            cookies_state_file: Some(cookies_state_file),
            load_cookies_on_open: true,
            ..Default::default()
        },
        session_root,
        user_data_dir,
    ));

    let open = service
        .open(BrowserOpenRequest::launch())
        .await
        .expect("launch open should succeed when cookies file is missing");

    assert_eq!(open.mode, BrowserSessionMode::Launch);
    assert_eq!(open.cookies_loaded, 0);
}

#[tokio::test]
async fn browser_sidecar_service_rejects_blank_attach_endpoint() {
    // 测试场景: attach 模式必须提供显式且非空的 endpoint。
    let fixture = BrowserFixture::new("openjarvis-browser-service-attach-error");
    let mut service = BrowserSidecarService::new(fixture.service_config(true));

    let error = service
        .open(BrowserOpenRequest {
            mode: BrowserSessionMode::Attach,
            cdp_endpoint: Some("   ".to_string()),
        })
        .await
        .expect_err("blank attach endpoint should fail");

    assert!(error.to_string().contains("non-empty `cdp_endpoint`"));
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
