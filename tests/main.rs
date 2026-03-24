use openjarvis::{agent::AgentWorker, config::AppConfig, router::ChannelRouter};

#[tokio::test]
async fn startup_components_build_from_default_config() {
    let config = AppConfig::default();
    let agent = AgentWorker::from_config(&config)
        .await
        .expect("agent should build");
    let _router = ChannelRouter::new(agent);
}
