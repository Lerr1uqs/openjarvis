use chrono::Utc;
use openjarvis::{
    command::{CommandInvocation, CommandRegistry},
    model::{IncomingMessage, ReplyTarget},
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

    let reply = registry
        .try_execute(&incoming)
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

    let reply = registry
        .try_execute(&incoming)
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

    let reply = registry
        .try_execute(&incoming)
        .await
        .expect("non-feishu command parse should succeed");

    assert!(reply.is_none());
}

#[tokio::test]
async fn unknown_slash_command_returns_failed_reply() {
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/unknown payload");

    let reply = registry
        .try_execute(&incoming)
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
    let matching = registry
        .try_execute(&build_incoming("/equal left left"))
        .await
        .expect("matching equal command should execute")
        .expect("matching equal command should be handled");
    let mismatching = registry
        .try_execute(&build_incoming("/equal left right"))
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

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_command".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_command".to_string(),
        user_name: None,
        content: content.to_string(),
        thread_id: Some("thread_command".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}
