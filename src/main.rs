//! Binary entrypoint that loads configuration, boots channels, and waits for shutdown signals.

use anyhow::Result;
use openjarvis::{agent::AgentWorker, config::AppConfig, router::ChannelRouter};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = AppConfig::load()?;
    if config.channel_config().feishu_config().dry_run {
        warn!("feishu.dry_run=true, outgoing messages will be logged instead of delivered");
    }

    let agent = AgentWorker::from_config(config.llm_config())?;
    let router = ChannelRouter::new(agent);

    router.register_channels(config.channel_config()).await?;
    info!(
        feishu_mode = config.channel_config().feishu_config().mode,
        "openjarvis server started"
    );

    shutdown_signal().await;
    Ok(())
}

fn init_tracing() {
    // Initialize tracing once and honor `RUST_LOG` when it is present.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
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
