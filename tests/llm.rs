use openjarvis::{
    config::LlmConfig,
    llm::{LlmRequest, build_provider},
};

#[tokio::test]
async fn mock_provider_returns_configured_response() {
    let config = LlmConfig {
        provider: "mock".to_string(),
        mock_response: "收到".to_string(),
        ..LlmConfig::default()
    };
    let provider = build_provider(&config).expect("mock provider should build");
    let reply = provider
        .generate(LlmRequest {
            system_prompt: "system".to_string(),
            user_message: "hello".to_string(),
        })
        .await
        .expect("mock provider should reply");

    assert_eq!(reply, "收到");
}

#[tokio::test]
async fn mock_llm_alias_builds_same_provider() {
    let config = LlmConfig {
        provider: "mock_llm".to_string(),
        mock_response: "pong".to_string(),
        ..LlmConfig::default()
    };
    let provider = build_provider(&config).expect("mock_llm alias should build");
    let reply = provider
        .generate(LlmRequest {
            system_prompt: "system".to_string(),
            user_message: "ping".to_string(),
        })
        .await
        .expect("mock_llm alias should reply");

    assert_eq!(reply, "pong");
}
