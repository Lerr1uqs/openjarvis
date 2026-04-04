use chrono::Utc;
use openjarvis::{
    compact::{
        COMPACTED_ASSISTANT_PREFIX, COMPACTED_USER_CONTINUE_MESSAGE, CompactManager,
        CompactSummary, StaticCompactProvider,
    },
    context::{ChatMessage, ChatMessageRole},
};
use std::sync::Arc;

#[tokio::test]
async fn compact_manager_replaces_messages_with_compacted_history() {
    // 测试场景: manager 应直接按 message 边界压缩全部输入消息，并输出固定两条 replacement messages。
    let now = Utc::now();
    let messages = vec![
        ChatMessage::new(ChatMessageRole::User, "需求", now),
        ChatMessage::new(ChatMessageRole::Assistant, "处理中", now),
        ChatMessage::new(ChatMessageRole::Assistant, "更多上下文", now),
    ];
    let manager = CompactManager::new(Arc::new(StaticCompactProvider::new(CompactSummary {
        compacted_assistant: "已完成与待完成".to_string(),
    })));

    let outcome = manager
        .compact_messages(&messages, now)
        .await
        .expect("compact should succeed")
        .expect("non-empty message list should compact");

    assert_eq!(outcome.source_message_count, 3);
    assert_eq!(outcome.compacted_messages.len(), 2);
    assert_eq!(
        outcome.compacted_messages[0].role,
        ChatMessageRole::Assistant
    );
    assert_eq!(
        outcome.compacted_messages[0].content,
        format!("{COMPACTED_ASSISTANT_PREFIX}{}", "已完成与待完成")
    );
    assert_eq!(outcome.compacted_messages[1].role, ChatMessageRole::User);
    assert_eq!(
        outcome.compacted_messages[1].content,
        COMPACTED_USER_CONTINUE_MESSAGE
    );
}

#[tokio::test]
async fn compact_manager_returns_none_for_empty_messages() {
    // 测试场景: 没有可 compact 的消息时，manager 应直接返回 None。
    let manager = CompactManager::new(Arc::new(StaticCompactProvider::new(CompactSummary {
        compacted_assistant: "state".to_string(),
    })));

    let outcome = manager
        .compact_messages(&[], Utc::now())
        .await
        .expect("empty-message compact check should succeed");

    assert!(outcome.is_none());
}
