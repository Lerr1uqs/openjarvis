use chrono::Utc;
use openjarvis::{
    compact::ContextBudgetReport,
    context::ContextTokenKind,
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::{
        ConversationThread, ThreadCompactToolProjection, ThreadContext, ThreadContextLocator,
        ThreadToolEvent, ThreadToolEventKind, derive_internal_thread_id,
    },
};
use serde_json::json;
use std::collections::HashMap;
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

#[test]
fn overwrite_active_history_replaces_old_turns_but_keeps_thread_identity() {
    // 测试场景: compact 写回 active history 时，应替换 turn 列表，但 thread id 不能漂移。
    let now = Utc::now();
    let mut thread = ConversationThread::new("default", now);
    let original_thread_id = thread.id;
    thread.store_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "old history", now)],
        now,
        now,
    );

    let mut compacted = thread.clone();
    compacted.turns = vec![openjarvis::thread::ConversationTurn::new(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ],
        now,
        now,
    )];
    thread.overwrite_active_history(&compacted);

    assert_eq!(thread.id, original_thread_id);
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].messages[0].content, "这是压缩后的上下文");
    assert_eq!(thread.turns[0].messages[1].content, "继续");
}

#[test]
fn thread_context_roundtrips_legacy_thread_and_preserves_runtime_layers() {
    // 测试场景: 旧的 ConversationThread 迁移到 ThreadContext 后，conversation/state 分层和兼容回写都必须保持一致。
    let now = Utc::now();
    let mut legacy = ConversationThread::new("thread_ext", now);
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };
    legacy.store_turn_state(
        Some("msg_compat".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
        vec!["demo".to_string(), "demo".to_string(), String::new()],
        vec![event],
    );

    let locator = ThreadContextLocator::new(
        Some("session-1".to_string()),
        "feishu",
        "ou_xxx",
        "thread_ext",
        derive_internal_thread_id("ou_xxx:feishu:thread_ext").to_string(),
    );
    let context = ThreadContext::from_conversation_thread(locator.clone(), legacy);
    let roundtrip = context.to_conversation_thread();

    assert_eq!(context.locator, locator);
    assert_eq!(context.turns.len(), 1);
    assert_eq!(context.load_toolsets(), vec!["demo".to_string()]);
    assert_eq!(context.load_tool_events().len(), 1);
    assert_eq!(roundtrip.external_thread_id, "thread_ext");
    assert_eq!(
        roundtrip.id,
        derive_internal_thread_id("ou_xxx:feishu:thread_ext")
    );
    assert_eq!(roundtrip.loaded_toolsets, vec!["demo".to_string()]);
    assert_eq!(roundtrip.tool_events.len(), 1);
}

#[test]
fn thread_context_store_turn_binds_pending_tool_events_and_clears_compact_projection() {
    // 测试场景: 当前轮累计的 pending tool event 必须在落 turn 时绑定 turn_id，同时清空本轮 compact projection。
    let now = Utc::now();
    let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_ext");
    let mut context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_ext",
            thread_id.to_string(),
        ),
        now,
    );
    context.set_compact_tool_projection(Some(ThreadCompactToolProjection {
        auto_compact: true,
        visible: true,
        budget_report: ContextBudgetReport::new(
            HashMap::from([
                (ContextTokenKind::System, 12),
                (ContextTokenKind::Chat, 64),
                (ContextTokenKind::VisibleTool, 20),
                (ContextTokenKind::ReservedOutput, 16),
            ]),
            256,
        ),
    }));
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("demo__echo".to_string());
        event.tool_call_id = Some("call_1".to_string());
        event
    };
    context.record_tool_event(event);

    let turn_id = context.store_turn(
        Some("msg_runtime".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
    );
    let stored_events = context.load_tool_events();

    assert!(context.pending_tool_events().is_empty());
    assert!(context.compact_tool_projection().is_none());
    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].turn_id, Some(turn_id));
    assert_eq!(stored_events[0].tool_call_id.as_deref(), Some("call_1"));
}
