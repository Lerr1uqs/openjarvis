use chrono::Utc;
use openjarvis::{
    config::{AppConfig, LLMConfig, install_global_config},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMRequest, build_provider, build_provider_from_global_config},
};
use std::{env::temp_dir, fs, path::PathBuf};
use uuid::Uuid;

#[tokio::test]
async fn mock_provider_returns_configured_response() {
    let config = LLMConfig {
        protocol: "mock".to_string(),
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
async fn mock_protocol_builds_same_provider_even_with_vendor_style_provider_name() {
    let config = LLMConfig {
        protocol: "mock".to_string(),
        provider: "mock_llm".to_string(),
        mock_response: "pong".to_string(),
        ..LLMConfig::default()
    };
    let provider = build_provider(&config).expect("mock protocol should build");
    let reply = provider
        .generate(LLMRequest {
            messages: build_messages("system", "ping"),
            tools: Vec::new(),
        })
        .await
        .expect("mock protocol should reply");

    assert_eq!(
        reply
            .message
            .expect("mock provider should return text")
            .content,
        "pong"
    );
}

#[tokio::test]
async fn provider_can_build_from_explicit_and_global_config_paths() {
    // 测试场景: build_provider 继续支持显式配置，主启动链路也可以改走全局配置便捷入口。
    let llm_config = LLMConfig {
        protocol: "mock".to_string(),
        mock_response: "from-global-config".to_string(),
        ..LLMConfig::default()
    };

    let explicit_provider =
        build_provider(&llm_config).expect("explicit provider should build without global config");
    let explicit_reply = explicit_provider
        .generate(LLMRequest {
            messages: build_messages("system", "hello"),
            tools: Vec::new(),
        })
        .await
        .expect("explicit provider should reply");
    assert_eq!(
        explicit_reply
            .message
            .expect("explicit provider should return text")
            .content,
        "from-global-config"
    );

    let app_config = AppConfig::builder_for_test()
        .llm(llm_config)
        .build()
        .expect("test app config should validate");
    install_global_config(app_config).expect("global config should install");

    let global_provider =
        build_provider_from_global_config().expect("global provider should build");
    let global_reply = global_provider
        .generate(LLMRequest {
            messages: build_messages("system", "hello"),
            tools: Vec::new(),
        })
        .await
        .expect("global provider should reply");
    assert_eq!(
        global_reply
            .message
            .expect("global provider should return text")
            .content,
        "from-global-config"
    );
}

#[test]
fn openai_compatible_provider_can_read_api_key_from_path() {
    let path = temp_dir().join(format!("openjarvis-api-key-{}.txt", Uuid::new_v4()));
    fs::write(&path, "sk-test-token\n").expect("api key file should be written");

    let config = LLMConfig {
        protocol: "openai".to_string(),
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
        protocol: "mock".to_string(),
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
        protocol: "anthropic".to_string(),
        provider: "claude".to_string(),
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

#[test]
fn kimi_k2_5_known_model_exposes_official_token_limits() {
    let config = LLMConfig {
        protocol: "openai".to_string(),
        provider: "ark".to_string(),
        model: "kimi-k2.5".to_string(),
        base_url: "https://ark.cn-beijing.volces.com/api/coding/v3".to_string(),
        ..LLMConfig::default()
    };

    assert_eq!(config.context_window_tokens(), 262144);
    assert_eq!(config.max_output_tokens(), 32768);
}

#[test]
fn zai_alias_builds_openai_compatible_provider() {
    // 测试场景: provider 应允许任意上游厂商名，只要 protocol 明确为 openai 兼容即可构建。
    let config = LLMConfig {
        protocol: "openai".to_string(),
        provider: "zai".to_string(),
        model: "glm-5".to_string(),
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
        api_key: "test-key".to_string(),
        ..LLMConfig::default()
    };

    build_provider(&config).expect("zai alias should build as openai-compatible provider");
}

fn build_messages(system_prompt: &str, user_message: &str) -> Vec<ChatMessage> {
    // 作用: 为 llm 单测构造最小结构化消息列表。
    // 参数: system_prompt 为系统提示词，user_message 为用户消息。
    vec![
        ChatMessage::new(ChatMessageRole::System, system_prompt, Utc::now()),
        ChatMessage::new(ChatMessageRole::User, user_message, Utc::now()),
    ]
}
