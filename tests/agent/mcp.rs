use openjarvis::agent::{McpRegistry, McpServerDefinition, McpTransport};

#[tokio::test]
async fn mcp_registry_registers_and_lists_servers() {
    let registry = McpRegistry::new();
    registry
        .register(McpServerDefinition {
            name: "filesystem".to_string(),
            transport: McpTransport::Stdio,
            endpoint: "uvx mcp-server-filesystem".to_string(),
        })
        .await
        .expect("mcp server should register");

    let servers = registry.list().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "filesystem");

    let server = registry
        .get("filesystem")
        .await
        .expect("registered server should be found");
    assert_eq!(server.transport, McpTransport::Stdio);
}
