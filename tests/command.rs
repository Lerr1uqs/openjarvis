use chrono::Utc;
use openjarvis::{
    command::{CommandInvocation, CommandRegistry},
    model::{IncomingMessage, ReplyTarget},
    thread::{
        Thread, ThreadContextLocator, ThreadToolEvent, ThreadToolEventKind,
        derive_internal_thread_id,
    },
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn command_parser_ignores_non_command_messages() {
    let parsed = CommandInvocation::parse("hello world").expect("non-command parse should succeed");

    assert!(parsed.is_none());
}

#[test]
fn command_parser_rejects_blank_command_name() {
    let error = CommandInvocation::parse("/   ").expect_err("blank command should be rejected");

    assert_eq!(error.to_string(), "command name is required");
}

#[test]
fn command_parser_preserves_raw_echo_arguments() {
    let parsed = CommandInvocation::parse("/echo keep   spacing  ")
        .expect("echo parse should succeed")
        .expect("message should be recognized as a command");

    assert_eq!(parsed.name(), "echo");
    assert_eq!(parsed.raw_arguments(), "keep   spacing  ");
    assert_eq!(
        parsed.arguments(),
        ["keep".to_string(), "spacing".to_string()]
    );
}

#[tokio::test]
async fn builtin_echo_command_returns_the_full_argument_payload() {
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/echo mirror this content");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("echo command should execute")
        .expect("echo command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][echo][SUCCESS]: mirror this content"
    );
}

#[tokio::test]
async fn builtin_clear_command_resets_thread_context_to_initial_state() {
    // 测试场景: /clear 应清空当前线程全部历史消息和线程级 runtime 状态。
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/clear");
    let now = Utc::now();
    let mut thread_context = build_thread_context();
    let event = {
        let mut event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
        event.toolset_name = Some("demo".to_string());
        event.tool_name = Some("load_toolset".to_string());
        event
    };
    thread_context.enable_auto_compact();
    thread_context.store_turn_state(
        Some("msg_history".to_string()),
        vec![openjarvis::context::ChatMessage::new(
            openjarvis::context::ChatMessageRole::User,
            "需要被清空的历史",
            now,
        )],
        now,
        now,
        vec!["demo".to_string()],
        vec![event],
    );
    thread_context.record_tool_event(ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now));

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("clear command should execute")
        .expect("clear command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][clear][SUCCESS]: cleared current thread `thread_command`; all chat messages and thread-scoped runtime state have been reset"
    );
    assert!(thread_context.load_messages().is_empty());
    assert!(thread_context.load_toolsets().is_empty());
    assert!(thread_context.load_tool_events().is_empty());
    assert!(thread_context.pending_tool_events().is_empty());
    assert!(!thread_context.auto_compact_enabled(false));
}

#[tokio::test]
async fn feishu_mention_prefix_is_removed_before_command_match() {
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("@_user_1 /echo zxf");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("feishu mention-prefixed command should execute")
        .expect("feishu mention-prefixed command should be handled");

    assert_eq!(reply.formatted_content(), "[Command][echo][SUCCESS]: zxf");
}

#[tokio::test]
async fn non_feishu_message_does_not_strip_at_prefix_for_command_match() {
    let registry = CommandRegistry::with_builtin_commands();
    let mut incoming = build_incoming("@_user_1 /echo zxf");
    incoming.channel = "cli".to_string();
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("non-feishu command parse should succeed");

    assert!(reply.is_none());
}

#[tokio::test]
async fn unknown_slash_command_returns_failed_reply() {
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/unknown payload");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("unknown command should still return a reply")
        .expect("unknown command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][unknown][FAILED]: unknown command"
    );
}

#[tokio::test]
async fn builtin_equal_command_handles_match_and_mismatch() {
    let registry = CommandRegistry::with_builtin_commands();
    let mut matching_thread_context = build_thread_context();
    let mut mismatching_thread_context = build_thread_context();
    let matching = registry
        .try_execute_with_thread_context(
            &build_incoming("/equal left left"),
            &mut matching_thread_context,
        )
        .await
        .expect("matching equal command should execute")
        .expect("matching equal command should be handled");
    let mismatching = registry
        .try_execute_with_thread_context(
            &build_incoming("/equal left right"),
            &mut mismatching_thread_context,
        )
        .await
        .expect("mismatching equal command should execute")
        .expect("mismatching equal command should be handled");

    assert_eq!(
        matching.formatted_content(),
        "[Command][equal][SUCCESS]: left == left"
    );
    assert_eq!(
        mismatching.formatted_content(),
        "[Command][equal][FAILED]: left != right"
    );
}

#[test]
fn slash_prefixed_messages_are_treated_as_thread_commands() {
    // 测试场景: 任何 `/...` 输入都会走命令分发，未注册命令也要返回 unknown command。
    let registry = CommandRegistry::with_builtin_commands();
    let echo_is_command = registry
        .is_command(&build_incoming("/echo hi"))
        .expect("echo command should parse");
    let auto_compact_is_command = registry
        .is_command(&build_incoming("/auto-compact status"))
        .expect("auto-compact command should parse");

    assert!(echo_is_command);
    assert!(auto_compact_is_command);
}

#[tokio::test]
async fn removed_auto_compact_command_returns_unknown_reply_without_mutating_thread() {
    // 测试场景: `/auto-compact` 不再允许通过命令修改线程开关，只返回 unknown command。
    let registry = CommandRegistry::with_builtin_commands();
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&build_incoming("/auto-compact on"), &mut thread_context)
        .await
        .expect("removed auto-compact command should still execute through registry")
        .expect("removed auto-compact command should be handled as unknown command");

    assert_eq!(
        reply.formatted_content(),
        "[Command][auto-compact][FAILED]: unknown command"
    );
    assert!(!thread_context.auto_compact_enabled(false));
}

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_command".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_command".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("thread_command".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn build_thread_context() -> Thread {
    let thread_id = derive_internal_thread_id("ou_command:feishu:thread_command");
    Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_command",
            "thread_command",
            thread_id.to_string(),
        ),
        Utc::now(),
    )
}
