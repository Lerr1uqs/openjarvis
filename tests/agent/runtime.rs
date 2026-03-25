use openjarvis::{agent::AgentRuntime, config::AppConfig};

use super::tool::mcp::demo_stdio_config;

#[tokio::test]
async fn default_runtime_starts_with_empty_registries() {
    let runtime = AgentRuntime::new();

    assert_eq!(runtime.hooks().len().await, 0);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.tools().mcp().list_servers().await.len(), 0);
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
    assert_eq!(runtime.tools().mcp().list_servers().await.len(), 0);
}

#[tokio::test]
async fn runtime_from_config_loads_tool_managed_mcp_servers() {
    let config = demo_stdio_config(false);
    let runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("runtime should build");

    let servers = runtime.tools().mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_stdio");
    assert!(!servers[0].enabled);
    assert_eq!(servers[0].tool_count, 0);
}
