use clap::Parser;
use openjarvis::{
    cli::{InternalMcpCommand, OpenJarvisCli},
    config::{AgentMcpServerTransportConfig, AppConfig, BUILTIN_MCP_SERVER_NAME},
};

#[test]
fn cli_parses_builtin_mcp_flag() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "--builtin-mcp"]);

    assert!(cli.builtin_mcp);
    assert!(cli.internal_mcp_command().is_none());
}

#[test]
fn cli_parses_internal_demo_http_command() {
    let cli = OpenJarvisCli::parse_from([
        "openjarvis",
        "internal-mcp",
        "demo-http",
        "--bind",
        "127.0.0.1:40001",
    ]);

    match cli.internal_mcp_command() {
        Some(InternalMcpCommand::DemoHttp { bind }) => {
            assert_eq!(bind, "127.0.0.1:40001");
        }
        other => panic!("unexpected parsed command: {other:?}"),
    }
}

#[test]
fn enabling_builtin_mcp_inserts_demo_stdio_server() {
    let mut config = AppConfig::default();
    config
        .enable_builtin_mcp("openjarvis")
        .expect("builtin mcp should be enabled");

    let server = config
        .agent_config()
        .tool_config()
        .mcp_config()
        .servers()
        .get(BUILTIN_MCP_SERVER_NAME)
        .expect("builtin MCP server should exist");

    assert!(server.enabled);
    match server.transport_config() {
        AgentMcpServerTransportConfig::Stdio { command, args, env } => {
            assert_eq!(command, "openjarvis");
            assert_eq!(
                args,
                &["internal-mcp".to_string(), "demo-stdio".to_string()]
            );
            assert!(env.is_empty());
        }
        other => panic!("unexpected builtin MCP transport: {other:?}"),
    }
}
