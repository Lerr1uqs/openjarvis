use chrono::Utc;
use openjarvis::{
    agent::AgentWorker,
    llm::MockLLMProvider,
    model::{IncomingMessage, ReplyTarget},
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::test]
async fn handle_message_generates_reply_and_session_metadata() {
    let agent = AgentWorker::new(
        Arc::new(MockLLMProvider::new("mock-reply")),
        "system prompt",
    );
    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: Some("tester".to_string()),
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

    let (tx, mut rx) = mpsc::channel(8);
    let output = agent
        .handle_message(incoming, tx)
        .await
        .expect("agent should handle message");
    let final_message = rx.recv().await.expect("agent should emit final reply");

    assert_eq!(output.reply, "mock-reply");
    assert_eq!(final_message.content, "mock-reply");
    assert_eq!(final_message.thread_id.as_deref(), Some("default"));
    assert_eq!(final_message.metadata["event_kind"], "TextOutput");
    assert_eq!(final_message.metadata["session_channel"], "feishu");
    assert_eq!(final_message.metadata["session_user_id"], "ou_xxx");
    assert_eq!(final_message.metadata["session_thread_id"], "default");
    assert_eq!(final_message.reply_to_message_id.as_deref(), Some("msg_1"));
}
