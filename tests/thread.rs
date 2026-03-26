use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::{ConversationThread, ThreadToolEvent, ThreadToolEventKind},
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

#[test]
fn retain_latest_messages_trims_across_turns_and_removes_empty_turns() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);

    thread.store_turn(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "message_0", now),
            ChatMessage::new(ChatMessageRole::Assistant, "message_1", now),
        ],
        now,
        now,
    );
    thread.store_turn(
        Some("msg_2".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "message_2", now),
            ChatMessage::new(ChatMessageRole::Assistant, "message_3", now),
        ],
        now,
        now,
    );
    thread.store_turn(
        Some("msg_3".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "message_4", now),
            ChatMessage::new(ChatMessageRole::Assistant, "message_5", now),
        ],
        now,
        now,
    );

    thread.retain_latest_messages(3);

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.turns[0].messages.len(), 1);
    assert_eq!(thread.turns[0].messages[0].content, "message_3");
    assert_eq!(
        thread
            .load_messages()
            .into_iter()
            .map(|message| message.content)
            .collect::<Vec<_>>(),
        vec![
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
        ]
    );
}

#[test]
fn retain_latest_messages_with_zero_clears_all_turns() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);

    thread.store_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "message_0",
            now,
        )],
        now,
        now,
    );

    thread.retain_latest_messages(0);

    assert!(thread.turns.is_empty());
    assert!(thread.load_messages().is_empty());
}

#[test]
fn store_turn_state_persists_loaded_toolsets_and_tool_events() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("browser".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };

    let turn_id = thread.store_turn_state(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
        vec!["browser".to_string()],
        vec![event],
    );

    assert_eq!(thread.load_toolsets(), vec!["browser".to_string()]);
    assert_eq!(thread.load_tool_events().len(), 1);
    assert_eq!(
        thread.load_tool_events()[0].toolset_name.as_deref(),
        Some("browser")
    );
    assert_eq!(thread.load_tool_events()[0].turn_id, Some(turn_id));
}
