use openjarvis::agent::AgentRuntime;

#[tokio::test]
async fn default_runtime_starts_with_empty_registries() {
    let runtime = AgentRuntime::new();

    assert_eq!(runtime.hooks().len().await, 0);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.mcp().list().await.len(), 0);
}
