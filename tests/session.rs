use chrono::Utc;
use openjarvis::{
    model::{IncomingMessage, ReplyTarget},
    session::{SessionKey, SessionManager},
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn begin_and_complete_turn_creates_session_state() {
    let manager = SessionManager::new();
    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: "hello".to_string(),
        thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };

    let pending = manager.begin_turn(&incoming, "system prompt").await;
    manager.complete_turn(&pending, "world").await;

    let session = manager
        .get_session(&SessionKey::from_incoming(&incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get("default")
        .expect("default thread should exist");

    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].assistant_message.as_deref(), Some("world"));
}
