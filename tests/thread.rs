#[path = "support/mod.rs"]
mod support;

use chrono::Utc;
use openjarvis::{
    agent::{FeaturePromptRebuilder, ToolRegistry},
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    thread::{
        Thread, ThreadContextLocator, ThreadFinalizedTurnStatus, ThreadRuntimeAttachment,
        ThreadToolEvent, ThreadToolEventKind, derive_internal_thread_id,
    },
};
use serde_json::json;
use std::sync::Arc;
use support::ThreadTestExt;

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

fn build_runtime_attachment(system_prompt: &str) -> ThreadRuntimeAttachment {
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    let rebuilder = Arc::new(FeaturePromptRebuilder::new(
        Arc::clone(&registry),
        AppConfig::default().agent_config().compact_config().clone(),
        system_prompt,
    ));
    let memory_repository = registry.memory_repository();
    ThreadRuntimeAttachment::new(registry, memory_repository, rebuilder, false, None)
}

#[test]
fn store_turn_preserves_tool_call_metadata_in_message_history() {
    // 测试场景: Thread 以 message 为最小持久化单位时，tool_call 元数据不能丢失。
    let now = Utc::now();
    let mut thread = build_thread("thread_messages");

    thread.commit_test_turn(
        Some("msg_1".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "hello", now),
            ChatMessage::new(ChatMessageRole::Assistant, "我先读取文件", now),
            ChatMessage::new(ChatMessageRole::Toolcall, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "Cargo.toml" }),
                },
            ]),
            ChatMessage::new(ChatMessageRole::ToolResult, "file-content", now)
                .with_tool_call_id("call_1"),
            ChatMessage::new(ChatMessageRole::Assistant, "读取完成", now),
        ],
        now,
        now,
    );

    let messages = thread.non_system_messages();

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[2].role, ChatMessageRole::Toolcall);
    assert_eq!(messages[2].tool_calls[0].id, "call_1");
    assert_eq!(messages[3].tool_call_id.as_deref(), Some("call_1"));
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

    thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt snapshot",
        now,
    )]);
    thread.enable_auto_compact();
    thread.commit_test_turn_with_state(
        Some("msg_clear".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "历史消息", now)],
        now,
        now,
        vec!["demo".to_string()],
        vec![event],
    );

    thread.clear_to_initial_state(now);

    assert!(thread.non_system_messages().is_empty());
    assert!(thread.load_toolsets().is_empty());
    assert!(thread.load_tool_events().is_empty());
    assert!(thread.system_messages().is_empty());
    assert!(!thread.auto_compact_enabled(false));
}

#[test]
fn record_tool_event_requires_active_turn() {
    // 测试场景: 没有 active turn 时记录 tool audit 必须直接失败，且不能留下任何 pending buffer。
    let now = Utc::now();
    let mut thread = build_thread("thread_events");
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("demo__echo".to_string());
        event.tool_call_id = Some("call_1".to_string());
        event
    };
    let error = thread
        .record_tool_event(event)
        .expect_err("tool audit without active turn should fail");

    assert!(error
        .to_string()
        .contains("cannot record tool event without one active turn"));
    assert!(thread.load_tool_events().is_empty());
}

#[test]
fn finalize_turn_persists_tool_events_without_turn_identity() {
    // 测试场景: active turn 内记录的 tool audit 在 finalize 后直接进入正式历史，且不再携带 turn_id。
    let now = Utc::now();
    let mut thread = build_thread("thread_events");
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
        event.tool_name = Some("demo__echo".to_string());
        event.tool_call_id = Some("call_1".to_string());
        event
    };

    thread.commit_test_turn_with_state(
        Some("msg_runtime".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
        now,
        now,
        Vec::new(),
        vec![event],
    );
    let stored_events = thread.load_tool_events();
    let serialized = serde_json::to_value(&stored_events[0]).expect("tool event should serialize");

    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].tool_call_id.as_deref(), Some("call_1"));
    assert!(serialized.get("turn_id").is_none());
}

#[test]
fn finalize_turn_failure_preserves_committed_messages_without_synthesizing_failure_message() {
    // 测试场景: finalize failure 只关闭 turn；已提交消息不能回滚，也不能自动再插入一条 failure message。
    let now = Utc::now();
    let mut thread = build_thread("thread_failed_turn");
    thread.commit_test_turn(
        Some("msg_seed".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "persisted history",
            now,
        )],
        now,
        now,
    );
    thread
        .begin_turn(Some("msg_fail".to_string()), now)
        .expect("failed turn should start");
    thread
        .append_open_turn_message(ChatMessage::new(
            ChatMessageRole::User,
            "current input",
            now,
        ))
        .expect("failed user message should enter current turn");
    thread
        .append_open_turn_message(ChatMessage::new(
            ChatMessageRole::Assistant,
            "partial reply",
            now,
        ))
        .expect("partial assistant reply should enter current turn");
    let mut event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now);
    event.tool_name = Some("demo__echo".to_string());
    event.tool_call_id = Some("call_fail_1".to_string());
    thread
        .record_tool_event(event)
        .expect("tool event should record inside failed turn");

    let finalized = thread
        .finalize_turn_failure("network exploded", now)
        .expect("failed turn should finalize");

    assert!(matches!(
        finalized.status,
        ThreadFinalizedTurnStatus::Failed { .. }
    ));
    assert_eq!(
        finalized
            .snapshot
            .non_system_messages()
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "persisted history".to_string(),
            "current input".to_string(),
            "partial reply".to_string(),
        ]
    );
    assert_eq!(finalized.snapshot.load_tool_events().len(), 1);
    assert_eq!(
        finalized.reply,
        "[openjarvis][agent_error] network exploded"
    );
    assert_eq!(finalized.snapshot.non_system_messages().len(), 3);
}

#[tokio::test]
async fn thread_snapshot_no_longer_serializes_dispatch_progress() {
    // 测试场景: Thread snapshot 只保留消息事实和 turn 生命周期，不再序列化 dispatch/ack 进度。
    let now = Utc::now();
    let mut thread = build_thread("thread_dispatch_state");
    thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread
        .begin_turn(Some("msg_dispatch".to_string()), now)
        .expect("turn should start");
    thread
        .push_message(ChatMessage::new(ChatMessageRole::Assistant, "partial", now))
        .await
        .expect("assistant message should append");

    thread
        .finalize_turn_success("partial", now)
        .expect("turn should finalize");
    assert!(!thread.has_active_turn());
    let serialized = serde_json::to_value(&thread).expect("thread should serialize");
    assert!(serialized["state"].get("dispatch").is_none());
    assert_eq!(
        thread
            .non_system_messages()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["partial"]
    );
}

#[test]
fn overwrite_active_history_replaces_message_snapshot() {
    // 测试场景: session/router 需要覆盖线程快照时，应直接替换 message 域和状态，而不是依赖 legacy turn 结构。
    let now = Utc::now();
    let mut thread = build_thread("thread_replace");
    thread.commit_test_turn(
        Some("msg_1".to_string()),
        vec![ChatMessage::new(ChatMessageRole::User, "old history", now)],
        now,
        now,
    );

    let mut compacted = build_thread("thread_replace");
    compacted.commit_test_turn(
        None,
        vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ],
        now,
        now,
    );
    thread.overwrite_active_history(&compacted);

    assert_eq!(thread.non_system_messages().len(), 2);
    assert_eq!(
        thread.non_system_messages()[0].content,
        "这是压缩后的上下文"
    );
    assert_eq!(thread.non_system_messages()[1].content, "继续");
}

#[test]
fn compaction_preserves_seeded_system_messages_at_front() {
    // 测试场景: compact 改写非 system 历史时，测试预置的 system message 仍应保留在前缀。
    let now = Utc::now();
    let mut thread = build_thread("thread_system_prefix");
    thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "stable system prompt",
        now,
    )]);
    thread
        .begin_turn(Some("msg_compact".to_string()), now)
        .expect("turn should start");
    thread
        .replace_non_system_messages_after_compaction(vec![
            ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
            ChatMessage::new(ChatMessageRole::User, "继续", now),
        ])
        .expect("compaction rewrite should succeed");

    assert_eq!(thread.system_messages().len(), 1);
    assert_eq!(thread.system_messages()[0].content, "stable system prompt");
    assert_eq!(thread.non_system_messages().len(), 2);
    assert_eq!(
        thread.non_system_messages()[0].content,
        "这是压缩后的上下文"
    );
    assert_eq!(thread.non_system_messages()[1].content, "继续");
}

#[tokio::test]
async fn thread_runtime_attachment_supports_idempotent_initialization() {
    // 测试场景: runtime service attach 到 Thread 后，应由 Thread 自己完成初始化且保持幂等。
    let mut thread = build_thread("thread_runtime_attach");
    thread.attach_runtime(build_runtime_attachment("stable system prompt"));

    let first = thread
        .ensure_initialized()
        .await
        .expect("first initialization should succeed");
    let second = thread
        .ensure_initialized()
        .await
        .expect("second initialization should stay idempotent");

    assert!(thread.has_runtime());
    assert!(thread.is_initialized());
    assert!(first);
    assert!(!second);
    assert!(
        thread
            .system_messages()
            .iter()
            .any(|message| message.content == "stable system prompt")
    );
    assert!(
        thread.system_messages().len() >= 2,
        "expected stable system prompt plus feature prompts"
    );
    assert!(
        thread
            .memory_repository()
            .expect("runtime attachment should expose memory repository")
            .memory_root()
            .ends_with(".openjarvis/memory")
    );
}

#[tokio::test]
async fn thread_ensure_initialized_backfills_existing_system_prefix_without_overwrite() {
    // 测试场景: 已存在旧 system prefix 的线程在 attach runtime 后，应只回填初始化标记而不覆盖原快照。
    let now = Utc::now();
    let mut thread = build_thread("thread_init_backfill");
    thread.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "legacy system snapshot",
        now,
    )]);
    thread.attach_runtime(build_runtime_attachment("new system prompt"));

    let changed = thread
        .ensure_initialized()
        .await
        .expect("legacy initialization backfill should succeed");

    assert!(!changed);
    assert!(thread.is_initialized());
    assert_eq!(thread.system_messages().len(), 1);
    assert_eq!(
        thread.system_messages()[0].content,
        "legacy system snapshot"
    );
}
