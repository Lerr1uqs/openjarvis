use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Uri},
    routing::post,
};
use openjarvis::{
    channels::{
        Channel, ChannelRegistration,
        feishu::{FeishuChannel, FeishuLongConnectionPayload, extract_text_message},
    },
    config::FeishuConfig,
    model::{OutgoingAttachment, OutgoingAttachmentKind, OutgoingMessage, ReplyTarget},
};
use serde_json::json;
use std::{env::temp_dir, fs, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRequest {
    path: String,
    query: Option<String>,
    authorization: Option<String>,
    content_type: Option<String>,
    body: String,
}

async fn record_request(
    state: Arc<Mutex<Vec<CapturedRequest>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) {
    state.lock().await.push(CapturedRequest {
        path: uri.path().to_string(),
        query: uri.query().map(|value| value.to_string()),
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string()),
        content_type: headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string()),
        body: String::from_utf8_lossy(&body).to_string(),
    });
}

async fn auth_handler(
    State(state): State<Arc<Mutex<Vec<CapturedRequest>>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Json<serde_json::Value> {
    record_request(state, uri, headers, body).await;
    Json(json!({
        "code": 0,
        "msg": "success",
        "tenant_access_token": "tenant_token",
        "expire": 7200
    }))
}

async fn upload_handler(
    State(state): State<Arc<Mutex<Vec<CapturedRequest>>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Json<serde_json::Value> {
    record_request(state, uri, headers, body).await;
    Json(json!({
        "code": 0,
        "msg": "success",
        "data": {
            "image_key": "img_uploaded"
        }
    }))
}

async fn message_handler(
    State(state): State<Arc<Mutex<Vec<CapturedRequest>>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Json<serde_json::Value> {
    record_request(state, uri, headers, body).await;
    Json(json!({
        "code": 0,
        "msg": "success"
    }))
}

#[test]
fn text_content_is_extracted_from_feishu_payload() {
    assert_eq!(
        extract_text_message("text", r#"{"text":"hello"}"#),
        "hello".to_string()
    );
}

#[test]
fn unsupported_message_type_returns_placeholder() {
    assert_eq!(
        extract_text_message("image", r#"{"image_key":"img_x"}"#),
        "[unsupported feishu message type: image]".to_string()
    );
}

#[test]
fn long_connection_payload_is_mapped_to_unified_model() {
    let channel = FeishuChannel::new(FeishuConfig::default());
    let incoming = channel.parse_long_connection_incoming(
        serde_json::from_value::<FeishuLongConnectionPayload>(json!({
            "event_id": "evt_ws_1",
            "sender_open_id": "ou_xxx",
            "sender_type": "user",
            "tenant_key": "tenant_xxx",
            "message_id": "om_xxx_ws",
            "chat_id": "oc_xxx",
            "thread_id": "omt_xxx",
            "chat_type": "group",
            "message_type": "text",
            "content": "{\"text\":\"hello\"}"
        }))
        .expect("long connection payload should deserialize"),
    );

    assert_eq!(incoming.channel, "feishu");
    assert_eq!(incoming.user_id, "ou_xxx");
    assert_eq!(incoming.content, "hello");
    assert_eq!(incoming.reply_target.receive_id, "oc_xxx");
    // Feishu `chat_id` is the stable conversation container and should drive OpenJarvis
    // external thread resolution even when Feishu also provides one topic `thread_id`.
    assert_eq!(incoming.external_thread_id.as_deref(), Some("oc_xxx"));
    assert_eq!(incoming.metadata["feishu_thread_id"], "omt_xxx");
}

#[test]
fn long_connection_payload_without_thread_id_uses_chat_id_as_external_thread() {
    let channel = FeishuChannel::new(FeishuConfig::default());
    let incoming = channel.parse_long_connection_incoming(
        serde_json::from_value::<FeishuLongConnectionPayload>(json!({
            "event_id": "evt_ws_2",
            "sender_open_id": "ou_xxx",
            "sender_type": "user",
            "tenant_key": "tenant_xxx",
            "message_id": "om_xxx_ws_2",
            "chat_id": "oc_xxx",
            "thread_id": null,
            "chat_type": "group",
            "message_type": "text",
            "content": "{\"text\":\"hello\"}"
        }))
        .expect("long connection payload should deserialize"),
    );

    // Bug regression: missing Feishu `thread_id` must not collapse conversations into the
    // OpenJarvis fallback `default` thread. `chat_id` is still the correct external thread id.
    assert_eq!(incoming.external_thread_id.as_deref(), Some("oc_xxx"));
    assert!(incoming.metadata["feishu_thread_id"].is_null());
}

#[tokio::test]
async fn outgoing_image_attachment_is_uploaded_and_sent_to_feishu() {
    // 测试场景: 出站消息带 image 附件时，Feishu channel 必须完成 token -> text -> upload -> image 发送闭环。
    let recorded_requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            post(auth_handler),
        )
        .route("/open-apis/im/v1/images", post(upload_handler))
        .route("/open-apis/im/v1/messages", post(message_handler))
        .with_state(Arc::clone(&recorded_requests));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server should bind");
    let address = listener
        .local_addr()
        .expect("test server should expose local address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });

    let image_path = temp_dir().join(format!("openjarvis-feishu-{}.png", Uuid::new_v4()));
    fs::write(&image_path, b"fake-image-content").expect("test image should be written");

    let mut config = FeishuConfig::default();
    config.open_base_url = format!("http://{address}");
    config.app_id = "app_test".to_string();
    config.app_secret = "secret_test".to_string();
    config.dry_run = false;
    config.auto_start_sidecar = false;

    let channel = Arc::new(FeishuChannel::new(config));
    let (incoming_tx, _incoming_rx) = mpsc::channel(1);
    let (outgoing_tx, outgoing_rx) = mpsc::channel(1);
    let start_result = channel
        .clone()
        .start(ChannelRegistration {
            incoming_tx,
            outgoing_rx,
        })
        .await;
    assert!(
        start_result
            .expect_err("disabled sidecar should stop normal channel startup")
            .to_string()
            .contains("auto_start_sidecar")
    );

    outgoing_tx
        .send(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: "feishu".to_string(),
            content: "图片已生成".to_string(),
            external_thread_id: Some("thread_1".to_string()),
            metadata: json!({ "event_kind": "TextOutput" }),
            reply_to_message_id: None,
            attachments: vec![OutgoingAttachment {
                kind: OutgoingAttachmentKind::Image,
                path: image_path.display().to_string(),
                mime_type: None,
            }],
            target: ReplyTarget {
                receive_id: "oc_xxx".to_string(),
                receive_id_type: "chat_id".to_string(),
            },
        })
        .await
        .expect("outgoing message should be sent to channel loop");

    timeout(Duration::from_secs(2), async {
        loop {
            if recorded_requests.lock().await.len() >= 4 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("feishu requests should be recorded");

    let requests = recorded_requests.lock().await.clone();
    assert_eq!(requests.len(), 4);

    assert_eq!(
        requests[0].path,
        "/open-apis/auth/v3/tenant_access_token/internal"
    );
    assert!(requests[0].body.contains("\"app_id\":\"app_test\""));
    assert!(requests[0].body.contains("\"app_secret\":\"secret_test\""));

    assert_eq!(requests[1].path, "/open-apis/im/v1/messages");
    assert_eq!(
        requests[1].query.as_deref(),
        Some("receive_id_type=chat_id")
    );
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer tenant_token")
    );
    assert!(requests[1].body.contains("\"msg_type\":\"text\""));
    assert!(requests[1].body.contains("图片已生成"));

    assert_eq!(requests[2].path, "/open-apis/im/v1/images");
    assert_eq!(
        requests[2].authorization.as_deref(),
        Some("Bearer tenant_token")
    );
    assert!(
        requests[2]
            .content_type
            .as_deref()
            .expect("upload content-type should exist")
            .starts_with("multipart/form-data")
    );
    assert!(requests[2].body.contains("image_type"));
    assert!(requests[2].body.contains("message"));
    assert!(requests[2].body.contains("fake-image-content"));

    assert_eq!(requests[3].path, "/open-apis/im/v1/messages");
    assert_eq!(
        requests[3].query.as_deref(),
        Some("receive_id_type=chat_id")
    );
    assert!(requests[3].body.contains("\"msg_type\":\"image\""));
    assert!(requests[3].body.contains("img_uploaded"));
    let text_send_body: serde_json::Value =
        serde_json::from_str(&requests[1].body).expect("text request body should be valid json");
    let image_send_body: serde_json::Value =
        serde_json::from_str(&requests[3].body).expect("image request body should be valid json");
    assert_ne!(text_send_body["uuid"], image_send_body["uuid"]);

    drop(outgoing_tx);
    let _ = fs::remove_file(&image_path);
    server.abort();
}
