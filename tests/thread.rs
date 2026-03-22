use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::ConversationThread,
};
use serde_json::json;

#[test]
fn append_and_complete_turn_updates_thread() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let turn_id = thread.append_user_turn(Some("msg_1".to_string()), "hello", now);

    assert_eq!(thread.turns.len(), 1);
    assert!(thread.turns[0].assistant_message.is_none());

    assert!(thread.complete_turn(turn_id, "world", now));
    assert_eq!(thread.turns[0].assistant_message.as_deref(), Some("world"));
}

#[test]
fn complete_turn_with_messages_preserves_tool_call_id() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let turn_id = thread.append_user_turn(Some("msg_1".to_string()), "hello", now);

    assert!(thread.complete_turn_with_messages(
        turn_id,
        vec![
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
    ));

    assert_eq!(
        thread.turns[0].assistant_message.as_deref(),
        Some("读取完成")
    );
    assert_eq!(thread.turns[0].messages.len(), 4);
    assert_eq!(thread.turns[0].messages[1].tool_calls[0].id, "call_1");
    assert_eq!(
        thread.turns[0].messages[2].tool_call_id.as_deref(),
        Some("call_1")
    );
}
