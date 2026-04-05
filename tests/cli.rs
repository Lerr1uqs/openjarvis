use clap::{Parser, error::ErrorKind};
use openjarvis::{
    cli::{InternalBrowserCommand, InternalMcpCommand, OpenJarvisCli, SkillCommand},
    config::{AgentMcpServerTransportConfig, AppConfig, BUILTIN_MCP_SERVER_NAME},
};

#[test]
fn cli_parses_builtin_mcp_flag() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "--builtin-mcp"]);

    assert!(!cli.debug);
    assert!(!cli.log_color);
    assert!(cli.builtin_mcp);
    assert!(cli.load_skills.is_empty());
    assert!(cli.internal_mcp_command().is_none());
}

#[test]
fn cli_parses_debug_and_log_color_flags() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "--debug", "--log-color"]);

    assert!(cli.debug);
    assert!(cli.log_color);
    assert!(!cli.builtin_mcp);
    assert!(cli.internal_mcp_command().is_none());
}

#[test]
fn cli_parses_repeated_load_skill_flags() {
    let cli = OpenJarvisCli::parse_from([
        "openjarvis",
        "--load-skill",
        "local_smoke_test",
        "--load-skill",
        "local_prompt_probe",
    ]);

    assert_eq!(cli.load_skills, ["local_smoke_test", "local_prompt_probe"]);
    assert!(!cli.builtin_mcp);
    assert!(cli.internal_mcp_command().is_none());
}

#[test]
fn cli_parses_public_skill_install_command() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "skill", "install", "acpx"]);

    assert!(matches!(
        cli.skill_command(),
        Some(SkillCommand::Install { name }) if name == "acpx"
    ));
    assert!(cli.internal_mcp_command().is_none());
}

#[test]
fn cli_parses_public_skill_uninstall_command() {
    let cli = OpenJarvisCli::parse_from(["openjarvis", "skill", "uninstall", "acpx"]);

    assert!(matches!(
        cli.skill_command(),
        Some(SkillCommand::Uninstall { name }) if name == "acpx"
    ));
}

#[test]
fn cli_rejects_unknown_skill_subcommand() {
    let error = OpenJarvisCli::try_parse_from(["openjarvis", "skill", "stall", "acpx"])
        .expect_err("unknown skill subcommand should fail");

    assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
    let rendered = error.to_string();
    assert!(rendered.contains("stall"));
    assert!(rendered.contains("install"));
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
fn cli_parses_internal_browser_smoke_command() {
    // 验证 smoke helper 的核心参数能按预期解析。
    let cli = OpenJarvisCli::parse_from([
        "openjarvis",
        "internal-browser",
        "smoke",
        "--url",
        "https://example.com",
        "--headless",
    ]);

    match cli.internal_browser_command() {
        Some(InternalBrowserCommand::Smoke { url, headless, .. }) => {
            assert_eq!(url, "https://example.com");
            assert!(*headless);
        }
        other => panic!("unexpected parsed browser command: {other:?}"),
    }
}

#[test]
fn cli_parses_internal_browser_script_command() {
    // 验证 script helper 可以接收步骤文件并开启 headless 模式。
    let cli = OpenJarvisCli::parse_from([
        "openjarvis",
        "internal-browser",
        "script",
        "--steps-file",
        "tmp/browser-steps.json",
        "--headless",
    ]);

    match cli.internal_browser_command() {
        Some(InternalBrowserCommand::Script {
            steps_file,
            headless,
            ..
        }) => {
            assert_eq!(
                steps_file,
                &std::path::PathBuf::from("tmp/browser-steps.json")
            );
            assert!(*headless);
        }
        other => panic!("unexpected parsed browser command: {other:?}"),
    }
}

#[test]
fn cli_parses_internal_browser_mock_sidecar_command() {
    // 验证测试用 mock-sidecar 子命令仍然可解析。
    let cli = OpenJarvisCli::parse_from(["openjarvis", "internal-browser", "mock-sidecar"]);

    assert!(matches!(
        cli.internal_browser_command(),
        Some(InternalBrowserCommand::MockSidecar)
    ));
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
