use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::ConversationThread,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn store_turn_updates_thread_and_preserves_final_assistant_message() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);

    thread.store_turn(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "hello", now),
            ChatMessage::new(ChatMessageRole::Assistant, "world", now),
        ],
        now,
        now,
    );

    assert_eq!(thread.turns.len(), 1);
    assert_eq!(
        thread.turns[0]
            .final_assistant_message()
            .map(|message| message.content.as_str()),
        Some("world")
    );
}

#[test]
fn load_messages_preserves_tool_call_metadata() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);

    thread.store_turn(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "hello", now),
            ChatMessage::new(ChatMessageRole::Assistant, "我先读取文件", now).with_tool_calls(
                vec![ChatToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "Cargo.toml" }),
                }],
            ),
            ChatMessage::new(ChatMessageRole::ToolResult, "file-content", now)
                .with_tool_call_id("call_1"),
            ChatMessage::new(ChatMessageRole::Assistant, "读取完成", now),
        ],
        now,
        now,
    );

    let messages = thread.load_messages();

    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].tool_calls[0].id, "call_1");
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_1"));
}

#[test]
fn load_or_create_turn_reuses_existing_turn_for_same_external_message_id() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let first_turn_id = Uuid::new_v4();

    let stored_turn_id = thread
        .load_or_create_turn(Some("msg_1".to_string()), first_turn_id, now, now)
        .id;
    let reused_turn_id = thread
        .load_or_create_turn(Some("msg_1".to_string()), Uuid::new_v4(), now, now)
        .id;

    assert_eq!(stored_turn_id, first_turn_id);
    assert_eq!(reused_turn_id, first_turn_id);
    assert_eq!(thread.turns.len(), 1);
}
