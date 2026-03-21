use chrono::Utc;
use openjarvis::{
    agent::AgentWorker,
    llm::MockLLMProvider,
    model::{IncomingMessage, ReplyTarget},
};
use serde_json::json;
use std::sync::Arc;
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

    let outgoing = agent
        .handle_message(incoming)
        .await
        .expect("agent should handle message")
        .expect("agent should reply");

    assert_eq!(outgoing.content, "mock-reply");
    assert_eq!(outgoing.thread_id.as_deref(), Some("default"));
    assert_eq!(outgoing.metadata["session_channel"], "feishu");
    assert_eq!(outgoing.metadata["session_user_id"], "ou_xxx");
    assert_eq!(outgoing.metadata["session_thread_id"], "default");
}
