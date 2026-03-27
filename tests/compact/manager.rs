use chrono::Utc;
use openjarvis::{
    compact::{
        COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactAllChatStrategy,
        CompactManager, CompactSummary, StaticCompactProvider,
    },
    context::{ChatMessage, ChatMessageRole},
    thread::{ConversationThread, ThreadToolEvent, ThreadToolEventKind},
};
use std::sync::Arc;

#[tokio::test]
async fn compact_manager_replaces_history_with_one_compacted_turn() {
    // 测试场景: manager 应该把被选中的历史 turn 替换成一个普通 compacted turn，并保留线程运行时元数据。
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let tool_event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("read".to_string());
        event
    };
    thread.store_turn_state(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "需求", now),
            ChatMessage::new(ChatMessageRole::Assistant, "处理中", now),
        ],
        now,
        now,
        vec!["browser".to_string()],
        vec![tool_event],
    );
    thread.store_turn_state(
        Some("msg_2".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "更多上下文",
            now,
        )],
        now,
        now,
        vec!["browser".to_string()],
        Vec::new(),
    );
    let manager = CompactManager::new(
        Arc::new(StaticCompactProvider::new(CompactSummary {
            compacted_assistant: "已完成与待完成".to_string(),
        })),
        Arc::new(CompactAllChatStrategy),
    );

    let outcome = manager
        .compact_thread(&thread, now)
        .await
        .expect("compact should succeed")
        .expect("non-empty thread should compact");

    assert_eq!(outcome.strategy_name, "compact_all_chat");
    assert_eq!(outcome.compacted_thread.turns.len(), 1);
    assert_eq!(outcome.compacted_turn.messages.len(), 2);
    assert_eq!(
        outcome.compacted_turn.messages[0].role,
        ChatMessageRole::Assistant
    );
    assert_eq!(
        outcome.compacted_turn.messages[0].content,
        format!("{COMPACTED_ASSISTANT_PREFIX}{}", "已完成与待完成")
    );
    assert_eq!(
        outcome.compacted_turn.messages[1].role,
        ChatMessageRole::User
    );
    assert_eq!(
        outcome.compacted_turn.messages[1].content,
        COMPACTED_USER_CONTINUE_MESSAGE
    );
    assert_eq!(
        outcome.compacted_thread.load_toolsets(),
        vec!["browser".to_string()]
    );
    assert_eq!(outcome.compacted_thread.load_tool_events().len(), 1);
}

#[tokio::test]
async fn compact_manager_returns_none_for_empty_thread() {
    // 测试场景: 空线程没有可 compact 的 chat 历史时，manager 应直接返回 None。
    let manager = CompactManager::new(
        Arc::new(StaticCompactProvider::new(CompactSummary {
            compacted_assistant: "state".to_string(),
        })),
        Arc::new(CompactAllChatStrategy),
    );
    let thread = ConversationThread::new("default", Utc::now());

    let outcome = manager
        .compact_thread(&thread, Utc::now())
        .await
        .expect("empty-thread compact check should succeed");

    assert!(outcome.is_none());
}
