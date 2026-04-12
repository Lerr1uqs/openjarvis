mod token_kind;

use chrono::Utc;
use openjarvis::context::{ChatMessage, ChatMessageRole, ChatToolCall};
use serde_json::json;

#[test]
fn chat_message_role_labels_match_prompt_contract() {
    // 测试场景: 统一消息协议的 role label 必须稳定，避免 prompt render 和 compact transcript 漂移。
    assert_eq!(ChatMessageRole::System.as_label(), "system");
    assert_eq!(ChatMessageRole::User.as_label(), "user");
    assert_eq!(ChatMessageRole::Assistant.as_label(), "assistant");
    assert_eq!(ChatMessageRole::Reasoning.as_label(), "reasoning");
    assert_eq!(ChatMessageRole::Toolcall.as_label(), "toolcall");
    assert_eq!(ChatMessageRole::ToolResult.as_label(), "tool_result");
}

#[test]
fn chat_message_preserves_tool_call_metadata() {
    // 测试场景: toolcall/tool_result 的 tool-call 关联信息必须原样保留。
    let message =
        ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now()).with_tool_calls(vec![
            ChatToolCall {
                id: "call_1".to_string(),
                name: "read".to_string(),
                arguments: json!({ "path": "config.yaml" }),
                provider_item_id: None,
            },
        ]);
    let result =
        ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now()).with_tool_call_id("call_1");
    let reasoning = ChatMessage::new(ChatMessageRole::Reasoning, "先推理", Utc::now())
        .with_provider_item_id("rsn_1");

    assert_eq!(message.tool_calls.len(), 1);
    assert_eq!(message.tool_calls[0].id, "call_1");
    assert_eq!(message.tool_calls[0].name, "read");
    assert_eq!(result.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(reasoning.provider_item_id.as_deref(), Some("rsn_1"));
}
