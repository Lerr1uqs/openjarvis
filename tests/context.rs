use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall, MessageContext},
    thread::ConversationThread,
};
use serde_json::json;

#[test]
fn context_renders_system_memory_and_chat() {
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

    let mut context = MessageContext::with_system_prompt("system prompt");
    context.push_memory("remember this");
    context.extend_from_thread(&thread);
    let rendered = context.render_for_llm();

    assert!(rendered.system_prompt.contains("system prompt"));
    assert!(rendered.system_prompt.contains("remember this"));
    assert!(rendered.user_message.contains("user: hello"));
    assert!(rendered.user_message.contains("assistant: world"));
}

#[test]
fn context_extend_from_thread_preserves_tool_call_metadata() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    thread.store_turn(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "读取配置", now),
            ChatMessage::new(ChatMessageRole::Assistant, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "config.yaml" }),
                },
            ]),
            ChatMessage::new(ChatMessageRole::ToolResult, "ok", now).with_tool_call_id("call_1"),
            ChatMessage::new(ChatMessageRole::Assistant, "完成", now),
        ],
        now,
        now,
    );

    let mut context = MessageContext::with_system_prompt("system prompt");
    context.extend_from_thread(&thread);
    let messages = context.as_messages();

    assert_eq!(messages.len(), 5);
    assert!(
        messages
            .iter()
            .any(|message| message.tool_calls.iter().any(|call| call.id == "call_1"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call_1"))
    );
}
