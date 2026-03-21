use anyhow::Result;
use openjarvis::{agent::AgentWorker, config::AppConfig, router::ChannelRouter};
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    // 作用: 启动 OpenJarvis 主进程并串起配置加载、router 注册和优雅退出。
    // 参数: 无，运行时依赖环境变量 OPENJARVIS_CONFIG 和当前工作目录下的配置文件。
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
    // 作用: 初始化全局 tracing 日志订阅器，优先读取环境变量中的日志级别。
    // 参数: 无，日志过滤规则来自默认环境变量。
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

async fn shutdown_signal() {
    // 作用: 等待 Ctrl+C 或系统终止信号，用于主进程优雅退出。
    // 参数: 无，内部监听当前平台支持的停止信号。
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
