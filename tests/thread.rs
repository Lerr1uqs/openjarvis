use chrono::Utc;
use openjarvis::{
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::{
        ConversationThread, ThreadContext, ThreadContextLocator, ThreadFeaturesSystemPrompt,
        ThreadToolEvent, ThreadToolEventKind, derive_internal_thread_id,
    },
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
fn clear_to_initial_state_resets_thread_context_layers() {
    // 测试场景: clear 要把 conversation、tool state、feature override 和 pending runtime state 一起恢复到初始态。
    let now = Utc::now();
    let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_clear");
    let mut context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_clear",
            thread_id.to_string(),
        ),
        now,
    );
    assert!(context.ensure_system_prompt_snapshot("system prompt snapshot", now));
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };
    context.enable_auto_compact();
    context.store_turn_state(
        Some("msg_clear".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "历史消息", now)],
        now,
        now,
        vec!["demo".to_string()],
        vec![event],
    );
    let pending_event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("demo__echo".to_string());
        event
    };
    context.record_tool_event(pending_event);

    context.clear_to_initial_state(now);

    assert!(context.load_messages().is_empty());
    assert!(context.load_toolsets().is_empty());
    assert!(context.load_tool_events().is_empty());
    assert!(context.pending_tool_events().is_empty());
    assert!(context.request_context_system_messages().is_empty());
    assert!(!context.compact_enabled(false));
    assert!(!context.auto_compact_enabled(false));
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
fn thread_context_store_turn_binds_pending_tool_events() {
    // 测试场景: 当前轮累计的 pending tool event 必须在落 turn 时绑定 turn_id。
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
    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].turn_id, Some(turn_id));
    assert_eq!(stored_events[0].tool_call_id.as_deref(), Some("call_1"));
}

#[test]
fn request_context_snapshot_does_not_leak_into_conversation_history() {
    // 测试场景: 线程级 request context 是线程元数据，不应混入 load_messages 或 legacy conversation history。
    let now = Utc::now();
    let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_request_context");
    let mut context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_request_context",
            thread_id.to_string(),
        ),
        now,
    );
    assert!(context.ensure_system_prompt_snapshot("stable system prompt", now));
    context.store_turn(
        Some("msg_request_context".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
    );

    let flattened_messages = context.load_messages();
    let legacy_thread = context.to_conversation_thread();

    assert_eq!(
        context.request_context_system_messages()[0].content,
        "stable system prompt"
    );
    assert_eq!(flattened_messages.len(), 1);
    assert!(
        flattened_messages
            .iter()
            .all(|message| message.content != "stable system prompt")
    );
    assert!(
        legacy_thread
            .load_messages()
            .iter()
            .all(|message| message.content != "stable system prompt")
    );
}

#[test]
fn thread_context_messages_exports_llm_view_in_thread_order() {
    // 测试场景: 对外导出的 LLM messages 必须由 ThreadContext 统一拼接，顺序固定为
    // persisted snapshot -> features_system_prompt -> live system -> live memory
    // -> persisted history -> live chat。
    let now = Utc::now();
    let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_export_messages");
    let mut context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_export_messages",
            thread_id.to_string(),
        ),
        now,
    );
    assert!(context.ensure_system_prompt_snapshot("stable system prompt", now));
    context.store_turn(
        Some("msg_export".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "persisted history",
            now,
        )],
        now,
        now,
    );
    let mut features_system_prompt = ThreadFeaturesSystemPrompt::default();
    features_system_prompt
        .toolset_catalog
        .push(ChatMessage::new(
            ChatMessageRole::System,
            "toolset catalog",
            now,
        ));
    features_system_prompt.skill_catalog.push(ChatMessage::new(
        ChatMessageRole::System,
        "skill catalog",
        now,
    ));
    features_system_prompt.auto_compact.push(ChatMessage::new(
        ChatMessageRole::System,
        "auto compact stable",
        now,
    ));
    context.rebuild_features_system_prompt(features_system_prompt);
    context.push_message(ChatMessage::new(
        ChatMessageRole::System,
        "runtime capacity prompt",
        now,
    ));
    context.replace_request_memory_messages(vec![ChatMessage::new(
        ChatMessageRole::Memory,
        "transient memory",
        now,
    )]);
    context.push_message(ChatMessage::new(ChatMessageRole::User, "current user", now));
    let exported = context.messages();

    assert_eq!(
        exported
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "stable system prompt".to_string(),
            "toolset catalog".to_string(),
            "skill catalog".to_string(),
            "auto compact stable".to_string(),
            "runtime capacity prompt".to_string(),
            "transient memory".to_string(),
            "persisted history".to_string(),
            "current user".to_string(),
        ]
    );
    assert_eq!(context.load_messages().len(), 1);
}

#[test]
fn rebuild_features_system_prompt_replaces_old_slots_without_touching_snapshot() {
    // 测试场景: features_system_prompt rebuild 只能替换静态 system prompt 槽位，不能改写初始化 snapshot。
    let now = Utc::now();
    let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_rebuild_features");
    let mut context = ThreadContext::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_xxx",
            "thread_rebuild_features",
            thread_id.to_string(),
        ),
        now,
    );
    assert!(context.ensure_system_prompt_snapshot("stable system prompt", now));

    let mut first_slots = ThreadFeaturesSystemPrompt::default();
    first_slots.toolset_catalog.push(ChatMessage::new(
        ChatMessageRole::System,
        "old toolset catalog",
        now,
    ));
    first_slots.auto_compact.push(ChatMessage::new(
        ChatMessageRole::System,
        "old auto compact prompt",
        now,
    ));
    context.rebuild_features_system_prompt(first_slots);

    let mut second_slots = ThreadFeaturesSystemPrompt::default();
    second_slots.skill_catalog.push(ChatMessage::new(
        ChatMessageRole::System,
        "new skill catalog",
        now,
    ));
    context.rebuild_features_system_prompt(second_slots);

    assert_eq!(
        context.request_context_system_messages()[0].content,
        "stable system prompt"
    );
    assert!(context.features_system_prompt().toolset_catalog.is_empty());
    assert!(context.features_system_prompt().auto_compact.is_empty());
    assert_eq!(
        context.features_system_prompt().skill_catalog[0].content,
        "new skill catalog"
    );
}
