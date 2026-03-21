use chrono::Utc;
use openjarvis::thread::ConversationThread;

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
