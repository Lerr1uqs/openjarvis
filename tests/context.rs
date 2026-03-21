use chrono::Utc;
use openjarvis::{context::MessageContext, thread::ConversationThread};

#[test]
fn context_renders_system_memory_and_chat() {
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let turn_id = thread.append_user_turn(Some("msg_1".to_string()), "hello", now);
    thread.complete_turn(turn_id, "world", now);

    let mut context = MessageContext::with_system_prompt("system prompt");
    context.push_memory("remember this");
    context.extend_from_thread(&thread);
    let rendered = context.render_for_llm();

    assert!(rendered.system_prompt.contains("system prompt"));
    assert!(rendered.system_prompt.contains("remember this"));
    assert!(rendered.user_message.contains("user: hello"));
    assert!(rendered.user_message.contains("assistant: world"));
}
