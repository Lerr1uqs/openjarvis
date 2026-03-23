use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    model::{IncomingMessage, ReplyTarget},
    session::{SessionKey, SessionManager, SessionStrategy},
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn store_and_load_turn_creates_session_state() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_1", "hello");
    let locator = manager.load_or_create_thread(&incoming).await;

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            vec![
                ChatMessage::new(ChatMessageRole::User, "hello", incoming.received_at),
                ChatMessage::new(ChatMessageRole::Assistant, "world", Utc::now()),
            ],
            incoming.received_at,
            Utc::now(),
        )
        .await;

    let history = manager.load_turn(&locator).await;
    let session = manager
        .get_session(&SessionKey::from_incoming(&incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("default thread should exist");

    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.external_thread_id, "default");
    assert_eq!(history.len(), 2);
    assert_eq!(
        thread.turns[0]
            .final_assistant_message()
            .map(|message| message.content.as_str()),
        Some("world")
    );
}

#[tokio::test]
async fn load_turn_keeps_tool_call_id_history() {
    let manager = SessionManager::new();
    let incoming = build_incoming("msg_1", "read config");
    let locator = manager.load_or_create_thread(&incoming).await;

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            vec![
                ChatMessage::new(ChatMessageRole::User, "read config", incoming.received_at),
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
            incoming.received_at,
            Utc::now(),
        )
        .await;

    let history = manager.load_turn(&locator).await;

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
}

#[tokio::test]
async fn strategy_keeps_only_latest_five_messages_per_turn() {
    let manager = SessionManager::with_strategy(SessionStrategy {
        max_messages_per_turn: 5,
    });
    let incoming = build_incoming("msg_1", "trim this");
    let locator = manager.load_or_create_thread(&incoming).await;

    manager
        .store_turn(
            &locator,
            incoming.external_message_id.clone(),
            (0..7)
                .map(|index| {
                    ChatMessage::new(
                        ChatMessageRole::Assistant,
                        format!("message_{index}"),
                        Utc::now(),
                    )
                })
                .collect(),
            incoming.received_at,
            Utc::now(),
        )
        .await;

    let session = manager
        .get_session(&SessionKey::from_incoming(&incoming))
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("default thread should exist");

    assert_eq!(thread.turns[0].messages.len(), 5);
    assert_eq!(thread.turns[0].messages[0].content, "message_2");
    assert_eq!(thread.turns[0].messages[4].content, "message_6");
}

#[tokio::test]
async fn load_or_create_thread_reuses_internal_uuid_for_same_external_thread() {
    let manager = SessionManager::new();
    let first_incoming = build_incoming_with_thread("msg_1", "hello", Some("ext_thread_1"));
    let second_incoming = build_incoming_with_thread("msg_2", "world", Some("ext_thread_1"));

    let first_locator = manager.load_or_create_thread(&first_incoming).await;
    let second_locator = manager.load_or_create_thread(&second_incoming).await;

    assert_eq!(first_locator.session_id, second_locator.session_id);
    assert_eq!(first_locator.external_thread_id, "ext_thread_1");
    assert_eq!(second_locator.external_thread_id, "ext_thread_1");
    assert_eq!(first_locator.thread_id, second_locator.thread_id);
}

fn build_incoming(message_id: &str, content: &str) -> IncomingMessage {
    build_incoming_with_thread(message_id, content, None)
}

fn build_incoming_with_thread(
    message_id: &str,
    content: &str,
    thread_id: Option<&str>,
) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: content.to_string(),
        thread_id: thread_id.map(|value| value.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}
