//! Binary entrypoint that loads configuration, boots channels, and waits for shutdown signals.

use anyhow::Result;
use clap::Parser;
use openjarvis::{
    agent::{
        AgentWorker,
        tool::{browser, mcp::demo},
    },
    cli::OpenJarvisCli,
    command::CommandRegistry,
    config::{AppConfig, SessionStoreBackend, install_global_config},
    logging,
    router::ChannelRouter,
    session::{MemorySessionStore, SessionManager, SessionStore, SqliteSessionStore},
};
use std::sync::Arc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = OpenJarvisCli::parse();
    // test-only
    if let Some(command) = cli.internal_mcp_command() {
        return demo::run_internal_demo_command(command).await;
    }
    if let Some(command) = cli.internal_browser_command() {
        return browser::run_internal_browser_command(command).await;
    }

    let _logging_guards = logging::init_tracing_from_default_config()?;
    let mut config = AppConfig::load()?;
    if cli.builtin_mcp {
        let executable = std::env::current_exe()?;
        config.enable_builtin_mcp(executable.to_string_lossy().into_owned())?;
        warn!("builtin demo MCP enabled via --builtin-mcp");
    }
    if config.channel_config().feishu_config().dry_run {
        warn!("feishu.dry_run=true, outgoing messages will be logged instead of delivered");
    }
    info!(
        llm_provider = %config.llm_config().provider,
        llm_model = %config.llm_config().model,
        context_window_tokens = config.llm_config().context_window_tokens(),
        max_output_tokens = config.llm_config().max_output_tokens(),
        "resolved llm token limits"
    );
    let config = install_global_config(config)?;

    let agent = AgentWorker::from_global_config().await?;
    if !cli.load_skills.is_empty() {
        // Test-only startup path: enable only the explicitly requested local skills for this
        // process so external manual verification can exercise skill loading deterministically.
        let enabled_skills = agent
            .runtime()
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

    let command_registry = CommandRegistry::with_builtin_commands();

    let session_store: Arc<dyn SessionStore> =
        match config.session_config().persistence_config().backend() {
            SessionStoreBackend::Memory => Arc::new(MemorySessionStore::new()),
            SessionStoreBackend::Sqlite => Arc::new(
                SqliteSessionStore::open(
                    config
                        .session_config()
                        .persistence_config()
                        .sqlite_config()
                        .path(),
                )
                .await?,
            ),
        };

    let mut router = ChannelRouter::builder()
        .agent(agent)
        .session_manager(SessionManager::with_store(session_store).await?)
        .command_registry(command_registry)
        .build()?;

    router.register_channels(config.channel_config()).await?;
    info!(
        feishu_mode = config.channel_config().feishu_config().mode,
        "openjarvis server started"
    );

    router.run_until_shutdown(shutdown_signal()).await
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
