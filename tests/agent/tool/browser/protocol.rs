use openjarvis::agent::tool::browser::{
    BrowserAriaSnapshotResult, BrowserCloseResult, BrowserConsoleEntry, BrowserConsoleLevel,
    BrowserConsoleResult, BrowserOpenRequest, BrowserRequestDiagnosticsQuery, BrowserRequestEntry,
    BrowserRequestResultKind, BrowserRequestsResult, BrowserSessionMode, BrowserSidecarRequest,
    BrowserSidecarRequestPayload, BrowserSidecarResponse, BrowserSidecarResponsePayload,
    BrowserSnapshotElement, BrowserSnapshotResult,
};

#[test]
fn browser_protocol_round_trips_snapshot_request() {
    // 验证带可选上限的 snapshot 请求可以完整编解码。
    let encoded = serde_json::to_string(&BrowserSidecarRequest::new(
        "req-1",
        BrowserSidecarRequestPayload::Snapshot {
            max_elements: Some(120),
        },
    ))
    .expect("request should serialize");
    let decoded: BrowserSidecarRequest =
        serde_json::from_str(&encoded).expect("request should deserialize");

    assert_eq!(decoded.id, "req-1");
    assert!(matches!(
        decoded.payload,
        BrowserSidecarRequestPayload::Snapshot {
            max_elements: Some(120)
        }
    ));
}

#[test]
fn browser_protocol_round_trips_open_request() {
    // 验证显式 attach open 请求可以完整编解码。
    let encoded = serde_json::to_string(&BrowserSidecarRequest::new(
        "req-open",
        BrowserSidecarRequestPayload::Open(BrowserOpenRequest::attach("http://127.0.0.1:9222")),
    ))
    .expect("open request should serialize");
    let decoded: BrowserSidecarRequest =
        serde_json::from_str(&encoded).expect("open request should deserialize");

    assert_eq!(decoded.id, "req-open");
    match decoded.payload {
        BrowserSidecarRequestPayload::Open(request) => {
            assert_eq!(request.mode, BrowserSessionMode::Attach);
            assert_eq!(
                request.cdp_endpoint.as_deref(),
                Some("http://127.0.0.1:9222")
            );
        }
        other => panic!("unexpected request payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_round_trips_aria_snapshot_request() {
    // 验证 ARIA snapshot 请求可以完整编解码。
    let encoded = serde_json::to_string(&BrowserSidecarRequest::new(
        "req-aria",
        BrowserSidecarRequestPayload::AriaSnapshot,
    ))
    .expect("aria snapshot request should serialize");
    let decoded: BrowserSidecarRequest =
        serde_json::from_str(&encoded).expect("aria snapshot request should deserialize");

    assert_eq!(decoded.id, "req-aria");
    assert!(matches!(
        decoded.payload,
        BrowserSidecarRequestPayload::AriaSnapshot
    ));
}

#[test]
fn browser_protocol_round_trips_requests_query_request() {
    // 验证 requests 诊断查询参数可以完整编解码。
    let encoded = serde_json::to_string(&BrowserSidecarRequest::new(
        "req-requests",
        BrowserSidecarRequestPayload::Requests(BrowserRequestDiagnosticsQuery::new(Some(3), true)),
    ))
    .expect("requests query should serialize");
    let decoded: BrowserSidecarRequest =
        serde_json::from_str(&encoded).expect("requests query should deserialize");

    assert_eq!(decoded.id, "req-requests");
    match decoded.payload {
        BrowserSidecarRequestPayload::Requests(query) => {
            assert_eq!(query.limit, Some(3));
            assert!(query.failed_only);
        }
        other => panic!("unexpected request payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_round_trips_snapshot_response_payload() {
    // 验证 richer snapshot 结果中的新增字段不会在协议编解码时丢失。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::success(
        "req-2",
        BrowserSidecarResponsePayload::Snapshot(BrowserSnapshotResult {
            url: "https://example.com".to_string(),
            title: "Example Domain".to_string(),
            snapshot_text: "snapshot".to_string(),
            elements: vec![BrowserSnapshotElement {
                reference: "1".to_string(),
                tag_name: "a".to_string(),
                role: "link".to_string(),
                label: "More information".to_string(),
                text: "More information".to_string(),
                selector: "body > a:nth-of-type(1)".to_string(),
                href: Some("https://example.com/more".to_string()),
                target: Some("_blank".to_string()),
                input_type: None,
                placeholder: None,
                section_hint: Some("main".to_string()),
                disabled: false,
            }],
            total_candidate_count: 1,
            truncated: false,
        }),
    ))
    .expect("response should serialize");
    let decoded: BrowserSidecarResponse =
        serde_json::from_str(&encoded).expect("response should deserialize");

    assert!(decoded.ok);
    match decoded.result.expect("result should exist") {
        BrowserSidecarResponsePayload::Snapshot(snapshot) => {
            assert_eq!(snapshot.title, "Example Domain");
            assert_eq!(snapshot.elements.len(), 1);
            assert_eq!(snapshot.elements[0].reference, "1");
            assert_eq!(
                snapshot.elements[0].href.as_deref(),
                Some("https://example.com/more")
            );
            assert_eq!(snapshot.total_candidate_count, 1);
            assert!(!snapshot.truncated);
        }
        other => panic!("unexpected response payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_round_trips_console_response_payload() {
    // 验证 console 诊断结果中的规范化字段不会在协议编解码时丢失。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::success(
        "req-console-result",
        BrowserSidecarResponsePayload::Console(BrowserConsoleResult {
            entries: vec![BrowserConsoleEntry {
                timestamp: "2026-04-19T12:00:00Z".to_string(),
                level: BrowserConsoleLevel::Warn,
                text: "mock warning".to_string(),
                page_url: "https://example.com".to_string(),
                location: None,
            }],
        }),
    ))
    .expect("console response should serialize");
    let decoded: BrowserSidecarResponse =
        serde_json::from_str(&encoded).expect("console response should deserialize");

    assert!(decoded.ok);
    match decoded.result.expect("result should exist") {
        BrowserSidecarResponsePayload::Console(result) => {
            assert_eq!(result.entries.len(), 1);
            assert_eq!(result.entries[0].page_url, "https://example.com");
            assert_eq!(result.entries[0].level, BrowserConsoleLevel::Warn);
        }
        other => panic!("unexpected response payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_round_trips_requests_response_payload() {
    // 验证 requests 诊断结果中的状态字段不会在协议编解码时丢失。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::success(
        "req-requests-result",
        BrowserSidecarResponsePayload::Requests(BrowserRequestsResult {
            entries: vec![BrowserRequestEntry {
                timestamp: "2026-04-19T12:00:00Z".to_string(),
                method: "GET".to_string(),
                url: "https://example.com/api".to_string(),
                resource_type: "xhr".to_string(),
                status: Some(500),
                result: BrowserRequestResultKind::HttpError,
            }],
        }),
    ))
    .expect("requests response should serialize");
    let decoded: BrowserSidecarResponse =
        serde_json::from_str(&encoded).expect("requests response should deserialize");

    assert!(decoded.ok);
    match decoded.result.expect("result should exist") {
        BrowserSidecarResponsePayload::Requests(result) => {
            assert_eq!(result.entries.len(), 1);
            assert_eq!(result.entries[0].status, Some(500));
            assert_eq!(
                result.entries[0].result,
                BrowserRequestResultKind::HttpError
            );
        }
        other => panic!("unexpected response payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_round_trips_aria_snapshot_response_payload() {
    // 验证 ARIA snapshot 结果中的文本负载不会在协议编解码时丢失。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::success(
        "req-aria-result",
        BrowserSidecarResponsePayload::AriaSnapshot(BrowserAriaSnapshotResult {
            url: "https://example.com".to_string(),
            title: "Example Domain".to_string(),
            aria_snapshot: "- document:\n  - heading \"Example Domain\"".to_string(),
        }),
    ))
    .expect("aria snapshot response should serialize");
    let decoded: BrowserSidecarResponse =
        serde_json::from_str(&encoded).expect("aria snapshot response should deserialize");

    assert!(decoded.ok);
    match decoded.result.expect("result should exist") {
        BrowserSidecarResponsePayload::AriaSnapshot(snapshot) => {
            assert_eq!(snapshot.url, "https://example.com");
            assert!(snapshot.aria_snapshot.contains("heading"));
        }
        other => panic!("unexpected response payload: {other:?}"),
    }
}

#[test]
fn browser_protocol_failure_response_preserves_error_code() {
    // 验证失败响应仍然保留 sidecar 错误码，方便上层归类处理。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::failure(
        "req-3",
        "missing_ref",
        "unknown browser ref",
    ))
    .expect("failure response should serialize");
    let decoded: BrowserSidecarResponse =
        serde_json::from_str(&encoded).expect("failure response should deserialize");

    assert!(!decoded.ok);
    assert_eq!(
        decoded.error.expect("error should exist").code,
        "missing_ref"
    );
}

#[test]
fn browser_protocol_successfully_serializes_close_payload() {
    // 验证 close 动作仍然使用统一的 action 字段编码。
    let encoded = serde_json::to_string(&BrowserSidecarResponse::success(
        "req-4",
        BrowserSidecarResponsePayload::Close(BrowserCloseResult {
            closed: true,
            mode: Some(BrowserSessionMode::Launch),
            exported_cookies_path: Some("/tmp/browser-cookies.json".to_string()),
            exported_cookie_count: Some(3),
        }),
    ))
    .expect("close response should serialize");

    assert!(encoded.contains("\"action\":\"close\""));
    assert!(encoded.contains("\"closed\":true"));
    assert!(encoded.contains("\"exported_cookie_count\":3"));
}
