use chrono::Utc;
use openjarvis::{
    command::{CommandInvocation, CommandRegistry},
    compact::ContextBudgetEstimator,
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole, ContextTokenKind},
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
async fn builtin_context_command_returns_thread_context_summary() {
    // 测试场景: `/context` 应返回当前线程 persisted messages 的总体占用摘要。
    let registry = CommandRegistry::with_builtin_commands();
    let now = Utc::now();
    let incoming = build_incoming("/context");
    let mut thread_context = build_thread_context();
    assert!(thread_context.ensure_system_prompt_snapshot("system prompt", now));
    thread_context.store_turn(
        Some("msg_context_summary".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "hello summary", now),
            ChatMessage::new(ChatMessageRole::Assistant, "assistant summary", now),
        ],
        now,
        now,
    );

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("context command should execute")
        .expect("context command should be handled");

    assert_eq!(
        reply.formatted_content(),
        format!(
            "[Command][context][SUCCESS]: {}",
            expected_context_summary(&thread_context)
        )
    );
}

#[tokio::test]
async fn builtin_context_role_command_returns_aggregated_role_breakdown() {
    // 测试场景: `/context role` 应按 ChatMessageRole 聚合，而不是逐条展开。
    let registry = CommandRegistry::with_builtin_commands();
    let now = Utc::now();
    let incoming = build_incoming("/context role");
    let mut thread_context = build_thread_context();
    assert!(thread_context.ensure_system_prompt_snapshot("system prompt", now));
    thread_context.store_turn(
        Some("msg_context_role".to_string()),
        vec![
            ChatMessage::new(
                ChatMessageRole::User,
                "first line\nsecond line with enough content to trigger preview truncation output",
                now,
            ),
            ChatMessage::new(ChatMessageRole::Assistant, "assistant role payload", now),
        ],
        now,
        now,
    );

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("context role command should execute")
        .expect("context role command should be handled");

    assert_eq!(
        reply.formatted_content(),
        format!(
            "[Command][context][SUCCESS]: {}",
            expected_context_role_report(&thread_context)
        )
    );
}

#[tokio::test]
async fn builtin_context_detail_command_lists_recent_persisted_messages() {
    // 测试场景: `/context detail 2` 应返回最近 2 条 persisted message 的逐条明细。
    let registry = CommandRegistry::with_builtin_commands();
    let now = Utc::now();
    let incoming = build_incoming("/context detail 2");
    let mut thread_context = build_thread_context();
    assert!(thread_context.ensure_system_prompt_snapshot("system prompt", now));
    thread_context.store_turn(
        Some("msg_context_detail".to_string()),
        vec![
            ChatMessage::new(ChatMessageRole::User, "detail user 1", now),
            ChatMessage::new(ChatMessageRole::Assistant, "detail assistant 2", now),
            ChatMessage::new(ChatMessageRole::ToolResult, "detail tool result 3", now),
        ],
        now,
        now,
    );

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("context detail command should execute")
        .expect("context detail command should be handled");

    assert_eq!(
        reply.formatted_content(),
        format!(
            "[Command][context][SUCCESS]: {}",
            expected_context_detail_report(&thread_context, 2)
        )
    );
}

#[tokio::test]
async fn builtin_context_detail_command_reports_empty_selection() {
    // 测试场景: 空线程执行 `/context detail` 时应返回空明细而不是报错。
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/context detail");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("empty context detail command should execute")
        .expect("empty context detail command should be handled");

    assert_eq!(
        reply.formatted_content(),
        format!(
            "[Command][context][SUCCESS]: {}",
            expected_context_detail_report(&thread_context, 20)
        )
    );
}

#[tokio::test]
async fn builtin_context_commands_do_not_mutate_thread_state() {
    // 测试场景: `/context`、`/context role` 与 `/context detail` 都是只读命令，不得修改线程消息或线程级状态。
    let registry = CommandRegistry::with_builtin_commands();
    let now = Utc::now();
    let mut thread_context = build_thread_context();
    let mut load_event = ThreadToolEvent::new(ThreadToolEventKind::LoadToolset, now);
    load_event.toolset_name = Some("demo".to_string());
    assert!(thread_context.ensure_system_prompt_snapshot("system prompt", now));
    thread_context.enable_auto_compact();
    thread_context.store_turn_state(
        Some("msg_context_readonly".to_string()),
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "readonly payload",
            now,
        )],
        now,
        now,
        vec!["demo".to_string()],
        vec![load_event],
    );
    thread_context.record_tool_event(ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, now));

    let messages_before = thread_context.messages();
    let toolsets_before = thread_context.load_toolsets();
    let tool_events_before = thread_context.load_tool_events();
    let pending_tool_events_before = thread_context.pending_tool_events().to_vec();
    let auto_compact_before = thread_context.auto_compact_enabled(false);

    registry
        .try_execute_with_thread_context(&build_incoming("/context"), &mut thread_context)
        .await
        .expect("context command should execute")
        .expect("context command should be handled");
    registry
        .try_execute_with_thread_context(&build_incoming("/context role"), &mut thread_context)
        .await
        .expect("context role command should execute")
        .expect("context role command should be handled");
    registry
        .try_execute_with_thread_context(&build_incoming("/context detail 1"), &mut thread_context)
        .await
        .expect("context detail command should execute")
        .expect("context detail command should be handled");

    assert_eq!(thread_context.messages(), messages_before);
    assert_eq!(thread_context.load_toolsets(), toolsets_before);
    assert_eq!(thread_context.load_tool_events(), tool_events_before);
    assert_eq!(
        thread_context.pending_tool_events(),
        pending_tool_events_before.as_slice()
    );
    assert_eq!(
        thread_context.auto_compact_enabled(false),
        auto_compact_before
    );
}

#[tokio::test]
async fn builtin_context_command_rejects_unknown_subcommand() {
    // 测试场景: `/context` 只支持空参数、`role` 和 `detail [count]`，其他子命令应返回 usage 错误。
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/context unexpected");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("invalid context command should execute")
        .expect("invalid context command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][context][FAILED]: usage: /context [role|detail [count]]"
    );
}

#[tokio::test]
async fn builtin_context_command_rejects_invalid_detail_count() {
    // 测试场景: `/context detail` 的 count 必须是合法整数。
    let registry = CommandRegistry::with_builtin_commands();
    let incoming = build_incoming("/context detail invalid");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("invalid detail count command should execute")
        .expect("invalid detail count command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][context][FAILED]: usage: /context [role|detail [count]]"
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

fn context_estimator() -> ContextBudgetEstimator {
    let config = AppConfig::default();
    ContextBudgetEstimator::from_config(config.llm_config(), config.agent_config().compact_config())
}

fn expected_context_summary(thread_context: &Thread) -> String {
    let estimator = context_estimator();
    let messages = thread_context.messages();
    let report = estimator.estimate(&messages, &[]);

    format!(
        "thread=`{external_thread_id}`\npersisted_messages={message_count}\ntotal_estimated_tokens={total_estimated_tokens}/{context_window_tokens} ({utilization_percent:.2}%)\nsystem_tokens={system_tokens}, chat_tokens={chat_tokens}, visible_tool_tokens={visible_tool_tokens}, reserved_output_tokens={reserved_output_tokens}",
        external_thread_id = thread_context.locator.external_thread_id,
        message_count = messages.len(),
        total_estimated_tokens = report.total_estimated_tokens,
        context_window_tokens = report.context_window_tokens,
        utilization_percent = report.utilization_ratio * 100.0,
        system_tokens = report.system_tokens(),
        chat_tokens = report.chat_tokens(),
        visible_tool_tokens = report.visible_tool_tokens(),
        reserved_output_tokens = report.reserved_output_tokens(),
    )
}

fn expected_context_role_report(thread_context: &Thread) -> String {
    let estimator = context_estimator();
    let messages = thread_context.messages();
    let context_window_tokens = estimator.context_window_tokens();
    let mut lines = vec![
        format!("thread=`{}`", thread_context.locator.external_thread_id),
        String::new(),
        "message_role".to_string(),
        "| role | tokens | window_ratio |".to_string(),
        "| --- | ---: | ---: |".to_string(),
    ];

    for role in ordered_chat_message_roles() {
        let role_tokens = estimate_tokens_for_role(&messages, &estimator, &role);
        let ratio_percent = role_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "| {role} | {role_tokens} | {ratio_percent:.2}% |",
            role = role.as_label(),
        ));
    }
    lines.push(String::new());
    lines.push("context_token_kind".to_string());
    lines.push("| kind | tokens | window_ratio |".to_string());
    lines.push("| --- | ---: | ---: |".to_string());
    let report = estimator.estimate(&messages, &[]);
    for kind in ContextTokenKind::ALL {
        let kind_tokens = report.tokens(kind);
        let ratio_percent = kind_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "| {kind} | {kind_tokens} | {ratio_percent:.2}% |",
            kind = kind.as_str(),
        ));
    }

    lines.join("\n")
}

fn expected_context_detail_report(thread_context: &Thread, requested_count: usize) -> String {
    let estimator = context_estimator();
    let messages = thread_context.messages();
    let context_window_tokens = estimator.context_window_tokens();
    let detail_count = requested_count.min(messages.len());
    let start_index = messages.len().saturating_sub(detail_count);
    let selected_messages = &messages[start_index..];
    let mut lines = vec![
        format!("thread=`{}`", thread_context.locator.external_thread_id),
        format!(
            "persisted_messages={}\ncontext_window_tokens={}\ndetail_count={}",
            messages.len(),
            context_window_tokens,
            detail_count,
        ),
    ];

    if selected_messages.is_empty() {
        lines.push("no persisted messages selected for detail output".to_string());
        return lines.join("\n");
    }

    lines.push(format!(
        "showing_message_range={}..{}",
        start_index + 1,
        start_index + detail_count,
    ));

    for (offset, message) in selected_messages.iter().enumerate() {
        let estimated_tokens = estimator.estimate_message(message);
        let ratio_percent = estimated_tokens as f64 / context_window_tokens as f64 * 100.0;
        lines.push(format!(
            "{index}. role={role} tokens={estimated_tokens} window_ratio={ratio_percent:.2}% preview=\"{preview}\"",
            index = start_index + offset + 1,
            role = message.role.as_label(),
            preview = message_preview(&message.content),
        ));
    }

    lines.join("\n")
}

fn ordered_chat_message_roles() -> [ChatMessageRole; 5] {
    [
        ChatMessageRole::System,
        ChatMessageRole::User,
        ChatMessageRole::Assistant,
        ChatMessageRole::Toolcall,
        ChatMessageRole::ToolResult,
    ]
}

fn estimate_tokens_for_role(
    messages: &[ChatMessage],
    estimator: &ContextBudgetEstimator,
    role: &ChatMessageRole,
) -> usize {
    messages
        .iter()
        .filter(|message| &message.role == role)
        .map(|message| estimator.estimate_message(message))
        .sum::<usize>()
}

fn message_preview(content: &str) -> String {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "<empty>".to_string();
    }

    let preview_limit = 48;
    let total_chars = normalized.chars().count();
    let mut preview = normalized
        .chars()
        .take(preview_limit)
        .collect::<String>()
        .replace('"', "'");
    if total_chars > preview_limit {
        preview.push_str("...");
    }
    preview
}
