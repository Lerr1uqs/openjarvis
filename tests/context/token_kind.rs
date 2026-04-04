use openjarvis::{
    compact::ContextBudgetReport,
    context::{ChatMessageRole, ContextTokenKind},
};
use std::collections::HashMap;

#[test]
fn context_token_kind_maps_chat_roles_to_budget_buckets() {
    // 测试场景: 不同消息角色需要落到稳定预算桶，避免后续统计口径漂移。
    assert_eq!(
        ContextTokenKind::for_chat_message_role(&ChatMessageRole::System),
        ContextTokenKind::System
    );
    assert_eq!(
        ContextTokenKind::for_chat_message_role(&ChatMessageRole::User),
        ContextTokenKind::Chat
    );
    assert_eq!(
        ContextTokenKind::for_chat_message_role(&ChatMessageRole::ToolResult),
        ContextTokenKind::Chat
    );
}

#[test]
fn context_budget_report_reads_bucket_tokens_by_enum() {
    // 测试场景: 报告应支持通过统一枚举读取各预算桶，供 prompt 和后续诊断复用。
    let report = ContextBudgetReport::new(
        HashMap::from([
            (ContextTokenKind::System, 10),
            (ContextTokenKind::Chat, 40),
            (ContextTokenKind::VisibleTool, 12),
            (ContextTokenKind::ReservedOutput, 16),
        ]),
        128,
    );

    assert_eq!(ContextTokenKind::System.as_str(), "system_tokens");
    assert_eq!(
        ContextTokenKind::VisibleTool.as_str(),
        "visible_tool_tokens"
    );
    assert_eq!(report.tokens(ContextTokenKind::System), 10);
    assert_eq!(report.tokens(ContextTokenKind::Chat), 40);
    assert_eq!(report.tokens(ContextTokenKind::VisibleTool), 12);
    assert_eq!(report.tokens(ContextTokenKind::ReservedOutput), 16);
}

#[test]
fn context_budget_report_serializes_aligned_token_map_as_flat_fields() {
    // 测试场景: enum->token 的 map 既要成为真源，也要保持外部 JSON 仍是扁平字段。
    let report = ContextBudgetReport::new(
        HashMap::from([
            (ContextTokenKind::System, 10),
            (ContextTokenKind::Chat, 40),
            (ContextTokenKind::ReservedOutput, 16),
        ]),
        128,
    );
    let value = serde_json::to_value(&report).expect("report should serialize");

    assert_eq!(value["system_tokens"], 10);
    assert_eq!(value["chat_tokens"], 40);
    assert_eq!(value["reserved_output_tokens"], 16);
    assert!(value.get("token_counts").is_none());
}
