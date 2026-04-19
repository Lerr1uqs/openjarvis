//! Binary entrypoint that loads configuration, boots channels, and waits for shutdown signals.

use anyhow::Result;
use clap::Parser;
use openjarvis::{
    agent::{AgentWorker, SkillRegistry},
    cli::OpenJarvisCli,
    cli_command::CliCommandRegistry,
    command::CommandRegistry,
    config::{AppConfig, SessionStoreBackend, install_global_config},
    logging,
    router::ChannelRouter,
    session::{MemorySessionStore, SessionManager, SessionStore, SqliteSessionStore},
};
use std::sync::Arc;
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = OpenJarvisCli::parse();
    let cli_command_registry = CliCommandRegistry::with_builtin_commands()?;
    if cli_command_registry.dispatch_from_cli(&cli).await? {
        return Ok(());
    }

    let _logging_guards =
        logging::init_tracing_from_default_config_with_cli(cli.debug, cli.log_color)?;
    debug!(
        debug_enabled = cli.debug,
        log_color = cli.log_color,
        "applied cli logging overrides"
    );
    if std::env::var_os("OPENJARVIS_CONFIG").is_none() {
        // Keep the hidden `--load-skill` smoke path fail-fast when startup is resolving the
        // workspace default config. An explicitly supplied config file should still be loaded so
        // logging and config-side effects remain observable in tests and manual verification.
        validate_startup_load_skills(&cli.load_skills).await?;
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
    let resolved_llm = config
        .llm_config()
        .resolve_active_provider()
        .expect("validated app config should expose one resolved active llm provider");
    info!(
        llm_protocol = resolved_llm.effective_protocol(),
        llm_provider = %resolved_llm.name,
        llm_model = %resolved_llm.model,
        context_window_tokens = resolved_llm.context_window_tokens(),
        max_output_tokens = resolved_llm.max_output_tokens(),
        header_count = resolved_llm.headers.len(),
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

    let command_registry =
        CommandRegistry::with_builtin_commands_and_tools(agent.runtime().tools());

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

async fn validate_startup_load_skills(load_skills: &[String]) -> Result<()> {
    if load_skills.is_empty() {
        return Ok(());
    }

    debug!(skills = ?load_skills, "validating startup local skills before app config load");
    let registry = SkillRegistry::new();
    let enabled_skills = registry.restrict_to(load_skills).await?;
    let enabled_skill_names = enabled_skills
        .iter()
        .map(|manifest| manifest.name.as_str())
        .collect::<Vec<_>>();
    info!(
        skills = ?enabled_skill_names,
        "validated startup local skills before app config load"
    );
    Ok(())
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
