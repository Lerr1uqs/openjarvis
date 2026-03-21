mod feishu;

use anyhow::Result;
use async_trait::async_trait;
use openjarvis::channels::{Channel, ChannelRegistration};
use std::sync::Arc;

struct DummyChannel;

#[async_trait]
impl Channel for DummyChannel {
    fn name(&self) -> &'static str {
        "dummy"
    }

    async fn start(self: Arc<Self>, _registration: ChannelRegistration) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn default_channel_hooks_return_ok() {
    let channel = DummyChannel;

    channel.on_start().await.expect("on_start should succeed");
    channel
        .check_health()
        .await
        .expect("check_health should succeed");
    channel.on_stop().await.expect("on_stop should succeed");
}
