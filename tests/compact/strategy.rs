use chrono::Utc;
use openjarvis::{
    compact::{CompactSourceHandling, CompactStrategy, CompactionPlan},
    context::{ChatMessage, ChatMessageRole},
    thread::{ConversationThread, ConversationTurn},
};

#[test]
fn compact_all_chat_plan_collects_every_turn_in_thread_order() {
    // 测试场景: 首版默认策略需要把当前线程的全部 active chat turn 都纳入 compact 输入。
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    thread.store_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "first", now)],
        now,
        now,
    );
    thread.store_turn(
        Some("msg_2".to_string()),
        vec![ChatMessage::new(ChatMessageRole::Assistant, "second", now)],
        now,
        now,
    );

    let plan = openjarvis::compact::CompactAllChatStrategy
        .build_plan(&thread)
        .expect("strategy should succeed")
        .expect("non-empty thread should produce a plan");
    let messages = plan
        .source_messages(&thread)
        .expect("source messages should resolve");

    assert_eq!(
        plan.source_turn_ids,
        thread.turns.iter().map(|turn| turn.id).collect::<Vec<_>>()
    );
    assert_eq!(
        messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn compaction_plan_rejects_non_contiguous_turn_replacement() {
    // 测试场景: 为了保持 active history 顺序稳定，当前替换计划只允许连续 turn 被一起替换。
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    thread.store_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "first", now)],
        now,
        now,
    );
    thread.store_turn(
        Some("msg_2".to_string()),
        vec![ChatMessage::new(ChatMessageRole::Assistant, "middle", now)],
        now,
        now,
    );
    thread.store_turn(
        Some("msg_3".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "last", now)],
        now,
        now,
    );
    let plan = CompactionPlan::new(
        vec![thread.turns[0].id, thread.turns[2].id],
        CompactSourceHandling::DropSource,
    )
    .expect("plan should build");
    let replacement_turn = ConversationTurn::new(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ],
        now,
        now,
    );

    let error = plan
        .apply(&thread, replacement_turn)
        .expect_err("non-contiguous selection should fail");

    assert!(format!("{error:#}").contains("contiguous slice"));
}
