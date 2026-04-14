use openjarvis::agent::tool::browser::{
    BrowserCloseResult, BrowserOpenRequest, BrowserSessionMode, BrowserSidecarRequest,
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
