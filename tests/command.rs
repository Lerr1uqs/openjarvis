#[path = "support/mod.rs"]
mod support;

use chrono::Utc;
use openjarvis::{
    agent::{
        FeatureResolver, MemoryRepository, ToolCallRequest, ToolRegistry,
        tool::browser::{
            BrowserProcessCommandSpec, BrowserRuntimeOptions, BrowserSessionManagerConfig,
            register_browser_toolset_with_config,
        },
    },
    command::{CommandInvocation, CommandRegistry},
    compact::ContextBudgetEstimator,
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole, ContextTokenKind},
    model::{IncomingMessage, ReplyTarget},
    session::{SessionManager, ThreadLocator},
    thread::{
        ChildThreadIdentity, DEFAULT_ASSISTANT_SYSTEM_PROMPT, DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT,
        Feature, Features, SubagentSpawnMode, Thread, ThreadAgent, ThreadAgentKind,
        ThreadContextLocator, ThreadRuntime, derive_internal_thread_id,
    },
};
use serde_json::json;
use std::{env::temp_dir, fs, sync::Arc};
use support::ThreadTestExt;
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
    thread_context.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread_context.append_persisted_messages_for_test(vec![
        ChatMessage::new(ChatMessageRole::User, "hello summary", now),
        ChatMessage::new(ChatMessageRole::Assistant, "assistant summary", now),
    ]);

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
    thread_context.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread_context.append_persisted_messages_for_test(vec![
        ChatMessage::new(
            ChatMessageRole::User,
            "first line\nsecond line with enough content to trigger preview truncation output",
            now,
        ),
        ChatMessage::new(ChatMessageRole::Assistant, "assistant role payload", now),
    ]);

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
    thread_context.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread_context.append_persisted_messages_for_test(vec![
        ChatMessage::new(ChatMessageRole::User, "detail user 1", now),
        ChatMessage::new(ChatMessageRole::Assistant, "detail assistant 2", now),
        ChatMessage::new(ChatMessageRole::ToolResult, "detail tool result 3", now),
    ]);

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
    thread_context.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread_context.enable_feature(Feature::AutoCompact);
    thread_context.append_persisted_messages_with_state_for_test(
        vec![ChatMessage::new(
            ChatMessageRole::User,
            "readonly payload",
            now,
        )],
        vec!["demo".to_string()],
    );

    let messages_before = thread_context.messages();
    let toolsets_before = thread_context.load_toolsets();
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
async fn builtin_new_command_reinitializes_thread_context_to_initialized_state() {
    // 测试场景: `/new` 应清掉普通历史和线程级运行态，但命令返回前必须恢复稳定初始化前缀。
    let registry = CommandRegistry::with_builtin_commands();
    let runtime = build_thread_runtime();
    let incoming = build_incoming("/new");
    let now = Utc::now();
    let mut thread_context = build_thread_context();
    runtime
        .initialize_thread(&mut thread_context, ThreadAgentKind::Main)
        .await
        .expect("main thread should initialize before /new");
    let system_message_count_before = thread_context.system_messages().len();
    thread_context.enable_feature(Feature::AutoCompact);
    thread_context.append_persisted_messages_with_state_for_test(
        vec![openjarvis::context::ChatMessage::new(
            openjarvis::context::ChatMessageRole::User,
            "需要被清空的历史",
            now,
        )],
        vec!["demo".to_string()],
    );

    let reply = registry
        .try_execute_with_thread_context_and_runtime(
            &incoming,
            &mut thread_context,
            Some(runtime.as_ref()),
        )
        .await
        .expect("new command should execute")
        .expect("new command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][new][SUCCESS]: reinitialized current thread `thread_command`; stable system prefix and thread-scoped runtime state have been rebuilt"
    );
    assert!(thread_context.is_initialized());
    assert_eq!(thread_context.thread_agent_kind(), ThreadAgentKind::Main);
    assert_eq!(
        thread_context.messages()[0].content,
        DEFAULT_ASSISTANT_SYSTEM_PROMPT.trim()
    );
    assert_eq!(
        thread_context.system_messages().len(),
        system_message_count_before
    );
    assert!(thread_context.non_system_messages().is_empty());
    assert!(thread_context.load_toolsets().is_empty());
    assert!(!thread_context.auto_compact_enabled(false));
}

#[tokio::test]
async fn builtin_new_command_keeps_browser_child_thread_identity() {
    // 测试场景: browser child thread 执行 `/new` 后仍然保留 browser kind 和 child identity。
    let registry = CommandRegistry::with_builtin_commands();
    let runtime = build_thread_runtime();
    let now = Utc::now();
    let child_identity =
        ChildThreadIdentity::new("parent-thread", "browser", SubagentSpawnMode::Persist);
    let mut thread_context = Thread::new(
        ThreadContextLocator::new(
            None,
            "feishu",
            "ou_command",
            "thread_browser_child",
            "thread_browser_child",
        )
        .with_child_thread(child_identity.clone()),
        now,
    );
    thread_context
        .persist_child_thread_identity(child_identity.clone())
        .await
        .expect("browser child identity should persist before /new");
    runtime
        .initialize_thread(&mut thread_context, ThreadAgentKind::Browser)
        .await
        .expect("browser child thread should initialize before /new");
    thread_context.append_persisted_messages_for_test(vec![ChatMessage::new(
        ChatMessageRole::User,
        "browser history before /new",
        now,
    )]);

    let reply = registry
        .try_execute_with_thread_context_and_runtime(
            &build_incoming("/new"),
            &mut thread_context,
            Some(runtime.as_ref()),
        )
        .await
        .expect("browser /new command should execute")
        .expect("browser /new command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][new][SUCCESS]: reinitialized current thread `thread_browser_child`; stable system prefix and thread-scoped runtime state have been rebuilt"
    );
    assert!(thread_context.is_initialized());
    assert_eq!(thread_context.thread_agent_kind(), ThreadAgentKind::Browser);
    assert_eq!(
        thread_context.child_thread_identity(),
        Some(&child_identity)
    );
    assert_eq!(
        thread_context.messages()[0].content,
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim()
    );
    assert!(thread_context.non_system_messages().is_empty());
}

#[tokio::test]
async fn builtin_new_command_reinitializes_attached_persist_child_threads() {
    // 测试场景: parent `/new` 必须把当前 parent 名下的 persist child thread 一起 reset/reinit。
    let registry = CommandRegistry::with_builtin_commands();
    let runtime = build_thread_runtime();
    let sessions = SessionManager::new();
    sessions.install_thread_runtime(runtime.clone());

    let seed_incoming = build_incoming("seed parent");
    let parent_locator = sessions
        .create_thread(&seed_incoming, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");
    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_locator = sessions
        .create_thread_at(
            &child_locator,
            seed_incoming.received_at,
            ThreadAgentKind::Browser,
        )
        .await
        .expect("persist child thread should resolve");

    {
        let mut child_thread = sessions
            .lock_thread(&child_locator, seed_incoming.received_at)
            .await
            .expect("child lock should resolve")
            .expect("child thread should exist");
        child_thread
            .push_message(ChatMessage::new(
                ChatMessageRole::User,
                "child history before /new",
                seed_incoming.received_at,
            ))
            .await
            .expect("child history should persist");
    }

    let mut parent_thread = sessions
        .lock_thread(&parent_locator, seed_incoming.received_at)
        .await
        .expect("parent lock should resolve")
        .expect("parent thread should exist");
    parent_thread.bind_request_runtime(sessions.clone());
    parent_thread
        .push_message(ChatMessage::new(
            ChatMessageRole::User,
            "parent history before /new",
            seed_incoming.received_at,
        ))
        .await
        .expect("parent history should persist");

    let reply = registry
        .try_execute_with_thread_context_and_runtime(
            &build_incoming("/new"),
            &mut parent_thread,
            Some(runtime.as_ref()),
        )
        .await
        .expect("parent /new should execute")
        .expect("parent /new should be handled");
    assert!(
        reply
            .formatted_content()
            .contains("[Command][new][SUCCESS]")
    );
    drop(parent_thread);

    let child_after_new = sessions
        .load_thread(&child_locator)
        .await
        .expect("child thread should load after /new")
        .expect("child thread should still exist after /new");
    assert!(child_after_new.is_initialized());
    assert_eq!(
        child_after_new.thread_agent_kind(),
        ThreadAgentKind::Browser
    );
    assert_eq!(
        child_after_new.messages()[0].content,
        DEFAULT_BROWSER_THREAD_SYSTEM_PROMPT.trim()
    );
    assert!(child_after_new.non_system_messages().is_empty());
}

#[tokio::test]
async fn builtin_new_command_requires_installed_thread_runtime() {
    // 测试场景: `/new` 若当前进程没有安装 thread runtime，应显式失败而不是留下半重置状态。
    let registry = CommandRegistry::with_builtin_commands();
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&build_incoming("/new"), &mut thread_context)
        .await
        .expect("new command without runtime should execute")
        .expect("new command without runtime should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][new][FAILED]: current process does not have one installed thread runtime; /new is unavailable"
    );
    assert!(!thread_context.is_initialized());
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
async fn removed_clear_command_returns_unknown_reply_without_mutating_thread() {
    // 测试场景: `/clear` 已被移除，执行时只返回 unknown command，不得再修改线程。
    let registry = CommandRegistry::with_builtin_commands();
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&build_incoming("/clear"), &mut thread_context)
        .await
        .expect("removed clear command should still execute through registry")
        .expect("removed clear command should be handled as unknown command");

    assert_eq!(
        reply.formatted_content(),
        "[Command][clear][FAILED]: unknown command"
    );
    assert!(thread_context.messages().is_empty());
    assert!(!thread_context.is_initialized());
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

#[tokio::test]
async fn browser_export_cookies_command_exports_current_session_state() {
    // 测试场景: `/browser-export-cookies <path>` 在 Browser 子线程中应复用当前 browser session 并导出 cookies 文件。
    let root = temp_dir().join(format!(
        "openjarvis-command-browser-export-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("command browser export root should exist");
    let artifact_root = root.join("artifacts");
    let export_path = root.join("state/browser-cookies.json");
    let tools = Arc::new(ToolRegistry::new());
    register_browser_toolset_with_config(
        &tools,
        BrowserSessionManagerConfig {
            process: BrowserProcessCommandSpec {
                executable: env!("CARGO_BIN_EXE_openjarvis").to_string(),
                args: vec!["internal-browser".to_string(), "mock-sidecar".to_string()],
                env: Default::default(),
            },
            runtime: BrowserRuntimeOptions {
                headless: true,
                keep_artifacts: true,
                ..Default::default()
            },
            artifact_root,
        },
    )
    .await
    .expect("browser toolset should register");

    let mut thread_context = build_browser_thread_context();
    tools
        .call_for_context(
            &mut thread_context,
            ToolCallRequest {
                name: "browser__navigate".to_string(),
                arguments: json!({ "url": "https://example.com" }),
            },
        )
        .await
        .expect("browser navigate should create session");

    let registry = CommandRegistry::with_builtin_commands_and_tools(Arc::clone(&tools));
    let incoming = build_incoming(&format!(
        "/browser-export-cookies {}",
        export_path.display()
    ));

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("browser export cookies command should execute")
        .expect("browser export cookies command should be handled");

    assert_eq!(
        reply.formatted_content(),
        format!(
            "[Command][browser-export-cookies][SUCCESS]: exported 0 cookies to {}",
            export_path.display()
        )
    );
    assert!(export_path.exists());

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn browser_export_cookies_command_rejects_missing_path_argument() {
    // 测试场景: `/browser-export-cookies` 缺少路径参数时应返回 usage，而不是静默成功。
    let registry = CommandRegistry::with_builtin_commands_and_tools(Arc::new(ToolRegistry::new()));
    let incoming = build_incoming("/browser-export-cookies");
    let mut thread_context = build_thread_context();

    let reply = registry
        .try_execute_with_thread_context(&incoming, &mut thread_context)
        .await
        .expect("browser export cookies usage command should execute")
        .expect("browser export cookies usage command should be handled");

    assert_eq!(
        reply.formatted_content(),
        "[Command][browser-export-cookies][FAILED]: usage: /browser-export-cookies <path>"
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

fn build_browser_thread_context() -> Thread {
    let mut thread_context = build_thread_context();
    thread_context.replace_thread_agent(ThreadAgent::from_kind(ThreadAgentKind::Browser));
    thread_context.replace_loaded_toolsets(vec!["browser".to_string()]);
    thread_context
}

fn build_thread_runtime() -> Arc<ThreadRuntime> {
    Arc::new(ThreadRuntime::with_feature_resolver(
        Arc::new(ToolRegistry::new()),
        Arc::new(MemoryRepository::new(".")),
        AppConfig::default().agent_config().compact_config().clone(),
        FeatureResolver::development_default(Features::default()),
    ))
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

fn ordered_chat_message_roles() -> [ChatMessageRole; 6] {
    [
        ChatMessageRole::System,
        ChatMessageRole::User,
        ChatMessageRole::Assistant,
        ChatMessageRole::Reasoning,
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
