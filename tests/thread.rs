use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::{
        Thread, ThreadContextLocator, ThreadToolEvent, ThreadToolEventKind,
        derive_internal_thread_id,
    },
};
use serde_json::json;

fn build_thread(external_thread_id: &str) -> Thread {
    let thread_id = derive_internal_thread_id(&format!("ou_xxx:feishu:{external_thread_id}"));
    Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            external_thread_id,
            thread_id.to_string(),
        ),
        Utc::now(),
    )
}

#[test]
fn store_turn_preserves_tool_call_metadata_in_message_history() {
    // 测试场景: Thread 以 message 为最小持久化单位时，tool_call 元数据不能丢失。
    let now = Utc::now();
    let mut thread = build_thread("thread_messages");

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
fn clear_to_initial_state_resets_persisted_and_runtime_state() {
    // 测试场景: clear 要同时清空历史消息、tool state、tool audit 和 feature override。
    let now = Utc::now();
    let mut thread = build_thread("thread_clear");
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };

    assert!(thread.ensure_system_prompt_snapshot("system prompt snapshot", now));
    thread.enable_auto_compact();
    thread.store_turn_state(
        Some("msg_clear".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "历史消息", now)],
        now,
        now,
        vec!["demo".to_string()],
        vec![event],
    );
    thread.record_tool_event(ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now));

    thread.clear_to_initial_state(now);

    assert!(thread.load_messages().is_empty());
    assert!(thread.load_toolsets().is_empty());
    assert!(thread.load_tool_events().is_empty());
    assert!(thread.pending_tool_events().is_empty());
    assert!(thread.system_prefix_messages().is_empty());
    assert!(!thread.auto_compact_enabled(false));
}

#[test]
fn store_turn_state_binds_pending_tool_events_to_commit_id() {
    // 测试场景: pending tool event 必须在 turn commit 时写入统一 turn_id。
    let now = Utc::now();
    let mut thread = build_thread("thread_events");
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("demo__echo".to_string());
        event.tool_call_id = Some("call_1".to_string());
        event
    };
    thread.record_tool_event(event);

    let turn_id = thread.store_turn(
        Some("msg_runtime".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
    );
    let stored_events = thread.load_tool_events();

    assert!(thread.pending_tool_events().is_empty());
    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].turn_id, Some(turn_id));
    assert_eq!(stored_events[0].tool_call_id.as_deref(), Some("call_1"));
}

#[test]
fn overwrite_active_history_replaces_message_snapshot() {
    // 测试场景: session/router 需要覆盖线程快照时，应直接替换 message 域和状态，而不是依赖 legacy turn 结构。
    let now = Utc::now();
    let mut thread = build_thread("thread_replace");
    thread.store_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "old history", now)],
        now,
        now,
    );

    let mut compacted = build_thread("thread_replace");
    compacted.store_turn(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ],
        now,
        now,
    );
    thread.overwrite_active_history(&compacted);

    assert_eq!(thread.load_messages().len(), 2);
    assert_eq!(thread.load_messages()[0].content, "这是压缩后的上下文");
    assert_eq!(thread.load_messages()[1].content, "继续");
}

#[test]
fn ensure_system_prefix_messages_only_initializes_once() {
    // 测试场景: 稳定 system messages 只能初始化一次，不能在后续请求中被覆盖。
    let now = Utc::now();
    let mut thread = build_thread("thread_system_prefix");

    assert!(thread.ensure_system_prefix_messages(&[ChatMessage::new(
        ChatMessageRole::System,
        "stable system prompt",
        now,
    )]));
    assert!(!thread.ensure_system_prefix_messages(&[ChatMessage::new(
        ChatMessageRole::System,
        "new system prompt",
        now,
    )]));
    assert_eq!(thread.system_prefix_messages().len(), 1);
    assert_eq!(
        thread.system_prefix_messages()[0].content,
        "stable system prompt"
    );
}
