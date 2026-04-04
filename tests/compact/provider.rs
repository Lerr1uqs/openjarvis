use chrono::Utc;
use openjarvis::{
    compact::{
        CompactProvider, CompactRequest, LLMCompactProvider, build_compact_prompt,
        render_chat_history,
    },
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    llm::MockLLMProvider,
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn render_chat_history_keeps_tool_annotations() {
    // 测试场景: compact prompt 需要保留 tool_call 标识，避免摘要时丢失关键上下文。
    let now = Utc::now();
    let rendered = render_chat_history(&[
        ChatMessage::new(ChatMessageRole::Assistant, "read config", now).with_tool_calls(vec![
            ChatToolCall {
                id: "call_1".to_string(),
                name: "read".to_string(),
                arguments: json!({ "path": "config.yaml" }),
            },
        ]),
        ChatMessage::new(ChatMessageRole::ToolResult, "ok", now).with_tool_call_id("call_1"),
        ChatMessage::new(ChatMessageRole::User, "task", now),
    ]);

    assert!(rendered.contains("tool_calls={id=call_1,name=read"));
    assert!(rendered.contains("[tool_call_id=call_1]"));
}

#[test]
fn compact_prompt_contains_rendered_history() {
    // 测试场景: 固定结构 prompt 只依赖 message transcript，不再依赖 turn id。
    let now = Utc::now();
    let request = CompactRequest::new(vec![ChatMessage::new(ChatMessageRole::User, "hello", now)])
        .expect("request should build");
    let prompt = build_compact_prompt(&request);

    assert!(!prompt.user_prompt.contains("source_turn_ids"));
    assert!(prompt.user_prompt.contains("[1][user] hello"));
}

#[tokio::test]
async fn compact_provider_parses_json_from_mock_provider() {
    // 测试场景: provider 需要把 LLM 的 JSON 回复解析成 compact 后的 assistant 上下文。
    let provider = LLMCompactProvider::new(Arc::new(MockLLMProvider::new(
        r#"{"compacted_assistant":"执行状态"}"#,
    )));
    let request = CompactRequest::new(vec![ChatMessage::new(
        ChatMessageRole::User,
        "hello",
        Utc::now(),
    )])
    .expect("request should build");

    let summary = provider
        .compact(request)
        .await
        .expect("json response should parse");

    assert_eq!(summary.compacted_assistant, "执行状态");
}

#[tokio::test]
async fn compact_provider_rejects_invalid_json_response() {
    // 测试场景: 如果 compact provider 没有返回约定 JSON，模块应立即报错而不是写入脏摘要。
    let provider = LLMCompactProvider::new(Arc::new(MockLLMProvider::new("not-json")));
    let request = CompactRequest::new(vec![ChatMessage::new(
        ChatMessageRole::User,
        "hello",
        Utc::now(),
    )])
    .expect("request should build");

    let error = provider
        .compact(request)
        .await
        .expect_err("invalid json should fail");

    assert!(format!("{error:#}").contains("invalid JSON"));
}
