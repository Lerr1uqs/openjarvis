use chrono::Utc;
use openjarvis::{
    agent::empty_tool_input_schema,
    compact::{ContextBudgetEstimator, ContextBudgetReport},
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole, ContextTokenKind},
};
use std::collections::HashMap;

#[test]
fn context_budget_estimator_splits_system_memory_chat_and_tools() {
    // 测试场景: 预算估算要覆盖完整请求，并分别统计 system、memory、chat 和 visible tools。
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  compact:
    enabled: true
    reserved_output_tokens: 32
llm:
  provider: "mock"
  context_window_tokens: 256
  tokenizer: "chars_div4"
"#,
    )
    .expect("config should parse");
    let estimator = ContextBudgetEstimator::from_config(
        config.llm_config(),
        config.agent_config().compact_config(),
    );
    let report = estimator.estimate(
        &[
            ChatMessage::new(ChatMessageRole::System, "system prompt", Utc::now()),
            ChatMessage::new(ChatMessageRole::Memory, "remember this", Utc::now()),
            ChatMessage::new(ChatMessageRole::User, "hello compact", Utc::now()),
        ],
        &[openjarvis::agent::ToolDefinition {
            name: "compact".to_string(),
            description: "compact history".to_string(),
            input_schema: empty_tool_input_schema(),
            source: openjarvis::agent::ToolSource::Builtin,
        }],
    );

    assert!(report.system_tokens() > 0);
    assert!(report.memory_tokens() > 0);
    assert!(report.chat_tokens() > 0);
    assert!(report.visible_tool_tokens() > 0);
    assert_eq!(report.reserved_output_tokens(), 32);
    assert_eq!(report.context_window_tokens, 256);
    assert!(report.total_estimated_tokens >= 32);
}

#[test]
fn context_budget_report_reaches_ratio_at_boundary() {
    // 测试场景: 预算占比判断在边界值上应按达到阈值处理，供 hard/soft threshold 共用。
    let report = ContextBudgetReport::new(
        HashMap::from([
            (ContextTokenKind::System, 10),
            (ContextTokenKind::Memory, 10),
            (ContextTokenKind::Chat, 40),
            (ContextTokenKind::VisibleTool, 20),
            (ContextTokenKind::ReservedOutput, 20),
        ]),
        100,
    );

    assert!(report.reaches_ratio(1.0));
    assert!(report.reaches_ratio(0.8));
    assert!(!report.reaches_ratio(1.1));
}
