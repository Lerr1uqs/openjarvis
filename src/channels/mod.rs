//! Channel abstraction and shared registration types for external messaging platforms.

pub mod feishu;

use crate::model::{IncomingMessage, OutgoingMessage};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct ChannelRegistration {
    pub incoming_tx: mpsc::Sender<IncomingMessage>,
    pub outgoing_rx: mpsc::Receiver<OutgoingMessage>,
}

#[async_trait]
pub trait Channel: Send + Sync {
    /// Return the stable channel name used for router registration and outbound dispatch.
    fn name(&self) -> &'static str;

    /// Run pre-start checks or initialization before the channel enters its main loop.
    async fn on_start(&self) -> Result<()> { // TODO: 也许不需要？
        Ok(())
    }

    /// Start the channel runtime with the bidirectional registration allocated by the router.
    async fn start(self: Arc<Self>, registration: ChannelRegistration) -> Result<()>;

    /// Check basic health after startup.
    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    #[allow(dead_code)]
    /// Stop the channel and release resources when the runtime shuts down.
    async fn on_stop(&self) -> Result<()> {
        Ok(())
    }
}
