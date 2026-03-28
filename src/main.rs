//! Binary entrypoint that loads configuration, boots channels, and waits for shutdown signals.

use anyhow::Result;
use clap::Parser;
use openjarvis::{
    agent::{
        AgentRuntime, AgentWorker,
        tool::{browser, mcp::demo},
    },
    cli::OpenJarvisCli,
    command::{CommandRegistry, register_runtime_commands},
    config::{AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, SessionStoreBackend},
    llm::build_provider,
    logging,
    router::ChannelRouter,
    session::{MemorySessionStore, SessionManager, SessionStrategy, SessionStore, SqliteSessionStore},
};
use std::sync::Arc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = OpenJarvisCli::parse();
    let _logging_guards = logging::init_tracing_from_default_config()?;
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
    let compact_runtime = runtime.compact_runtime();
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
    let mut command_registry = CommandRegistry::with_builtin_commands();
    register_runtime_commands(
        &mut command_registry,
        config.agent_config().compact_config().enabled(),
        config.agent_config().compact_config().auto_compact(),
        compact_runtime,
    )?;
    // TODO: 修改为链式builder DEFAULT_ASSISTANT_SYSTEM_PROMPT在new中默认 但是提供注入api
    let agent = AgentWorker::builder()
        .llm(build_provider(config.llm_config())?)
        .runtime(runtime)
        .system_prompt(DEFAULT_ASSISTANT_SYSTEM_PROMPT)
        .llm_config(config.llm_config().clone())
        .compact_config(config.agent_config().compact_config().clone())
        .build()?;
    let session_strategy = if config.agent_config().compact_config().enabled() {
        SessionStrategy {
            max_messages_per_thread: usize::MAX,
        }
    } else {
        SessionStrategy::default()
    };
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
        .session_manager(SessionManager::with_store(session_store, session_strategy).await?)
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
