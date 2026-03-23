use chrono::Utc;
use openjarvis::{
    config::LLMConfig,
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMRequest, build_provider},
};
use std::{env::temp_dir, fs, path::PathBuf};
use uuid::Uuid;

#[tokio::test]
async fn mock_provider_returns_configured_response() {
    let config = LLMConfig {
        provider: "mock".to_string(),
        mock_response: "收到".to_string(),
        ..LLMConfig::default()
    };
    let provider = build_provider(&config).expect("mock provider should build");
    let reply = provider
        .generate(LLMRequest {
            messages: build_messages("system", "hello"),
            tools: Vec::new(),
        })
        .await
        .expect("mock provider should reply");

    assert_eq!(
        reply
            .message
            .expect("mock provider should return text")
            .content,
        "收到"
    );
}

#[tokio::test]
async fn mock_llm_alias_builds_same_provider() {
    let config = LLMConfig {
        provider: "mock_llm".to_string(),
        mock_response: "pong".to_string(),
        ..LLMConfig::default()
    };
    let provider = build_provider(&config).expect("mock_llm alias should build");
    let reply = provider
        .generate(LLMRequest {
            messages: build_messages("system", "ping"),
            tools: Vec::new(),
        })
        .await
        .expect("mock_llm alias should reply");

    assert_eq!(
        reply
            .message
            .expect("mock provider should return text")
            .content,
        "pong"
    );
}

#[test]
fn openai_compatible_provider_can_read_api_key_from_path() {
    let path = temp_dir().join(format!("openjarvis-api-key-{}.txt", Uuid::new_v4()));
    fs::write(&path, "sk-test-token\n").expect("api key file should be written");

    let config = LLMConfig {
        provider: "deepseek".to_string(),
        model: "deepseek-chat".to_string(),
        base_url: "https://api.deepseek.com/v1".to_string(),
        api_key_path: path.clone(),
        ..LLMConfig::default()
    };

    build_provider(&config).expect("provider should build from api_key_path");
    fs::remove_file(path).expect("api key file should be removed");
}

#[tokio::test]
async fn mock_provider_does_not_require_api_key_path() {
    let config = LLMConfig {
        provider: "mock".to_string(),
        mock_response: "still-mock".to_string(),
        api_key_path: PathBuf::from("Z:/this/path/should/not/be/read.txt"),
        ..LLMConfig::default()
    };

    let provider = build_provider(&config).expect("mock provider should ignore api_key_path");
    let reply = provider
        .generate(LLMRequest {
            messages: build_messages("system", "hello"),
            tools: Vec::new(),
        })
        .await
        .expect("mock provider should reply");

    assert_eq!(
        reply
            .message
            .expect("mock provider should return text")
            .content,
        "still-mock"
    );
}

#[tokio::test]
async fn anthropic_provider_builds_but_generate_is_not_implemented() {
    let config = LLMConfig {
        provider: "anthropic".to_string(),
        model: "claude-3-7-sonnet".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        api_key: "test-key".to_string(),
        ..LLMConfig::default()
    };

    let provider = build_provider(&config).expect("anthropic placeholder should build");
    let error = provider
        .generate(LLMRequest {
            messages: build_messages("system", "hello"),
            tools: Vec::new(),
        })
        .await
        .expect_err("anthropic placeholder should not generate yet");

    assert!(
        error
            .to_string()
            .contains("provider protocol `anthropic` is not implemented yet")
    );
}

fn build_messages(system_prompt: &str, user_message: &str) -> Vec<ChatMessage> {
    // 作用: 为 llm 单测构造最小结构化消息列表。
    // 参数: system_prompt 为系统提示词，user_message 为用户消息。
    vec![
        ChatMessage::new(ChatMessageRole::System, system_prompt, Utc::now()),
        ChatMessage::new(ChatMessageRole::User, user_message, Utc::now()),
    ]
}
