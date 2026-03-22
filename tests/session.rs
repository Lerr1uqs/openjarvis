use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
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

#[tokio::test]
async fn next_turn_context_keeps_tool_call_id_history() {
    let manager = SessionManager::new();
    let first = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: "read config".to_string(),
        thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };
    let first_pending = manager.begin_turn(&first, "system prompt").await;
    manager
        .complete_turn_with_messages(
            &first_pending,
            vec![
                ChatMessage::new(ChatMessageRole::Assistant, "", Utc::now()).with_tool_calls(vec![
                    ChatToolCall {
                        id: "call_1".to_string(),
                        name: "read".to_string(),
                        arguments: json!({ "path": "config.yaml" }),
                    },
                ]),
                ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now())
                    .with_tool_call_id("call_1"),
                ChatMessage::new(ChatMessageRole::Assistant, "done", Utc::now()),
            ],
            Utc::now(),
        )
        .await;

    let second = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_2".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: "what happened".to_string(),
        thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };

    let second_pending = manager.begin_turn(&second, "system prompt").await;
    let history = second_pending.context.as_messages();

    assert!(
        history
            .iter()
            .any(|message| message.tool_calls.iter().any(|call| call.id == "call_1"))
    );
    assert!(
        history
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call_1"))
    );
    assert_eq!(
        history.last().map(|message| message.content.as_str()),
        Some("what happened")
    );
}
