use openjarvis::{agent::AgentRuntime, config::AppConfig};

#[tokio::test]
async fn default_runtime_starts_with_empty_registries() {
    let runtime = AgentRuntime::new();

    assert_eq!(runtime.hooks().len().await, 0);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.mcp().list().await.len(), 0);
}

#[tokio::test]
async fn runtime_from_config_loads_configured_hooks() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  hook:
    notification: ["echo", "hello"]
llm:
  provider: "mock"
"#,
    )
    .expect("config should parse");
    let runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("runtime should build");

    assert_eq!(runtime.hooks().len().await, 1);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.mcp().list().await.len(), 0);
}
