use chrono::Utc;
use openjarvis::{
    command::{CommandInvocation, CommandRegistry, register_runtime_commands},
    compact::{CompactRuntimeManager, CompactScopeKey},
    model::{IncomingMessage, ReplyTarget},
    thread::{ThreadContext, ThreadContextLocator, derive_internal_thread_id},
};
use serde_json::json;
use std::sync::Arc;
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
fn all_registered_commands_are_treated_as_thread_commands() {
    let compact_runtime = Arc::new(CompactRuntimeManager::new());
    let mut registry = CommandRegistry::with_builtin_commands();
    register_runtime_commands(&mut registry, false, false, compact_runtime)
        .expect("runtime command should register");

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
async fn auto_compact_command_requires_resolved_thread_context() {
    // 测试场景: 所有命令都必须绑定到某个线程上下文，不能再走无线程入口。
    let compact_runtime = Arc::new(CompactRuntimeManager::new());
    let mut registry = CommandRegistry::with_builtin_commands();
    register_runtime_commands(&mut registry, false, false, compact_runtime)
        .expect("runtime command should register");

    let error = registry
        .try_execute(&build_incoming("/auto-compact on"))
        .await
        .expect_err("command should require a resolved thread context");

    assert!(
        error
            .to_string()
            .contains("all commands require a resolved thread context")
    );
}

#[tokio::test]
async fn auto_compact_command_can_enable_and_report_status_for_current_thread() {
    // 测试场景: 即使静态 compact 默认关闭，/auto-compact on 也应能在线程级启用并返回确认消息。
    let compact_runtime = Arc::new(CompactRuntimeManager::new());
    let mut registry = CommandRegistry::with_builtin_commands();
    register_runtime_commands(&mut registry, false, false, Arc::clone(&compact_runtime))
        .expect("runtime command should register");
    let incoming_on = build_incoming("/auto-compact on");
    let mut thread_context = build_thread_context();

    let enabled = registry
        .try_execute_with_thread_context(&incoming_on, &mut thread_context)
        .await
        .expect("auto-compact on should execute")
        .expect("auto-compact on should be handled");
    let status = registry
        .try_execute_with_thread_context(
            &build_incoming("/auto-compact status"),
            &mut thread_context,
        )
        .await
        .expect("auto-compact status should execute")
        .expect("auto-compact status should be handled");

    assert_eq!(
        enabled.formatted_content(),
        "[Command][auto-compact][SUCCESS]: auto-compact enabled for current thread `thread_command`; future turns will expose `compact` and context capacity prompts"
    );
    assert_eq!(
        status.formatted_content(),
        "[Command][auto-compact][SUCCESS]: auto-compact is enabled for current thread `thread_command`"
    );
    assert!(thread_context.compact_enabled(false));
    assert!(thread_context.auto_compact_enabled(false));
    #[allow(deprecated)]
    {
        let scope = CompactScopeKey::from_incoming(&incoming_on);
        assert!(compact_runtime.compact_enabled(&scope, false).await);
        assert!(compact_runtime.auto_compact_enabled(&scope, false).await);
    }
}

#[tokio::test]
async fn auto_compact_command_off_restores_disabled_status_for_current_thread() {
    // 测试场景: /auto-compact off 应关闭当前线程的 runtime override，并让 status 变回 disabled。
    let mut registry = CommandRegistry::with_builtin_commands();
    let compact_runtime = Arc::new(CompactRuntimeManager::new());
    register_runtime_commands(&mut registry, false, false, Arc::clone(&compact_runtime))
        .expect("runtime command should register");
    let incoming_on = build_incoming("/auto-compact on");
    let mut thread_context = build_thread_context();

    let _enabled = registry
        .try_execute_with_thread_context(&incoming_on, &mut thread_context)
        .await
        .expect("auto-compact on should execute")
        .expect("auto-compact on should be handled");
    let disabled = registry
        .try_execute_with_thread_context(&build_incoming("/auto-compact off"), &mut thread_context)
        .await
        .expect("auto-compact off should execute")
        .expect("auto-compact off should be handled");
    let status = registry
        .try_execute_with_thread_context(
            &build_incoming("/auto-compact status"),
            &mut thread_context,
        )
        .await
        .expect("auto-compact status should execute")
        .expect("auto-compact status should be handled");

    assert_eq!(
        disabled.formatted_content(),
        "[Command][auto-compact][SUCCESS]: auto-compact disabled for current thread `thread_command`; future turns will stop exposing `compact` and context capacity prompts"
    );
    assert_eq!(
        status.formatted_content(),
        "[Command][auto-compact][SUCCESS]: auto-compact is disabled for current thread `thread_command`"
    );
    assert!(!thread_context.compact_enabled(false));
    assert!(!thread_context.auto_compact_enabled(false));
    #[allow(deprecated)]
    {
        let scope = CompactScopeKey::from_incoming(&incoming_on);
        assert!(!compact_runtime.compact_enabled(&scope, false).await);
        assert!(!compact_runtime.auto_compact_enabled(&scope, false).await);
    }
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

fn build_thread_context() -> ThreadContext {
    let thread_id = derive_internal_thread_id("ou_command:feishu:thread_command");
    ThreadContext::new(
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
