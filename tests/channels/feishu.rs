use openjarvis::{
    channels::feishu::{FeishuChannel, FeishuLongConnectionPayload, extract_text_message},
    config::FeishuConfig,
};
use serde_json::json;

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
    assert_eq!(incoming.external_thread_id.as_deref(), Some("omt_xxx"));
}

#[test]
fn long_connection_payload_without_thread_id_keeps_none() {
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

    assert_eq!(incoming.external_thread_id, None);
}
