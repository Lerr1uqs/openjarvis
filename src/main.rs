//! Binary entrypoint that loads configuration, boots channels, and waits for shutdown signals.

use anyhow::Result;
use clap::Parser;
use openjarvis::{
    agent::{
        AgentRuntime, AgentWorker,
        tool::{browser, mcp::demo},
    },
    cli::OpenJarvisCli,
    config::{AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT},
    llm::build_provider,
    router::ChannelRouter,
};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = OpenJarvisCli::parse();
    init_tracing();
    // test-only
    if let Some(command) = cli.internal_mcp_command() {
        return demo::run_internal_demo_command(command).await;
    }
    if let Some(command) = cli.internal_browser_command() {
        return browser::run_internal_browser_command(command).await;
    }

    let mut config = AppConfig::load()?;
    if cli.builtin_mcp {
        let executable = std::env::current_exe()?;
        config.enable_builtin_mcp(executable.to_string_lossy().into_owned())?;
        warn!("builtin demo MCP enabled via --builtin-mcp");
    }
    if config.channel_config().feishu_config().dry_run {
        warn!("feishu.dry_run=true, outgoing messages will be logged instead of delivered");
    }

    let runtime = AgentRuntime::from_config(config.agent_config()).await?;
    if !cli.load_skills.is_empty() {
        // Test-only startup path: enable only the explicitly requested local skills for this
        // process so external manual verification can exercise skill loading deterministically.
        let enabled_skills = runtime
            .tools()
            .skills()
            .restrict_to(&cli.load_skills)
            .await?;
        let enabled_skill_names = enabled_skills
            .iter()
            .map(|manifest| manifest.name.as_str())
            .collect::<Vec<_>>();
        info!(skills = ?enabled_skill_names, "loaded local skills from startup flags");
    }

    let agent = AgentWorker::with_runtime(
        build_provider(config.llm_config())?,
        DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        runtime,
    );
    let mut router = ChannelRouter::new(agent);

    router.register_channels(config.channel_config()).await?;
    info!(
        feishu_mode = config.channel_config().feishu_config().mode,
        "openjarvis server started"
    );

    router.run_until_shutdown(shutdown_signal()).await
}

fn init_tracing() {
    // Initialize tracing once and honor `RUST_LOG` when it is present.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

async fn shutdown_signal() {
    // Wait for a supported shutdown signal so the process can exit cleanly.
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
