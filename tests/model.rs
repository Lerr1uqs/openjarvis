use chrono::Utc;
use openjarvis::model::{IncomingAttachment, IncomingMessage, OutgoingMessage, ReplyTarget};
use serde_json::json;
use uuid::Uuid;

#[test]
fn message_models_roundtrip_with_serde() {
    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: Some("tester".to_string()),
        content: "hello".to_string(),
        thread_id: Some("thread_1".to_string()),
        received_at: Utc::now(),
        metadata: json!({ "source": "test" }),
        attachments: vec![IncomingAttachment {
            name: "demo.txt".to_string(),
            url: Some("https://example.com/demo.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
        }],
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };

    let outgoing = OutgoingMessage {
        id: Uuid::new_v4(),
        channel: "feishu".to_string(),
        content: "reply".to_string(),
        thread_id: Some("thread_1".to_string()),
        metadata: json!({ "kind": "reply" }),
        reply_to_message_id: Some("msg_1".to_string()),
        target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };

    let incoming_value = serde_json::to_value(&incoming).expect("incoming should serialize");
    let outgoing_value = serde_json::to_value(&outgoing).expect("outgoing should serialize");

    let decoded_incoming: IncomingMessage =
        serde_json::from_value(incoming_value).expect("incoming should deserialize");
    let decoded_outgoing: OutgoingMessage =
        serde_json::from_value(outgoing_value).expect("outgoing should deserialize");

    assert_eq!(decoded_incoming.content, "hello");
    assert_eq!(decoded_incoming.attachments.len(), 1);
    assert_eq!(decoded_outgoing.content, "reply");
    assert_eq!(
        decoded_outgoing.reply_to_message_id.as_deref(),
        Some("msg_1")
    );
}
