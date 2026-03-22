//! Message router that wires channels to the agent worker with per-channel outbound queues.
//!
//! The current implementation keeps message handling simple by awaiting one inbound message
//! to completion inside each channel loop. This is intentionally conservative for the first
//! end-to-end version. A later refactor should switch to long-lived router<->agent channels
//! so the router can multiplex inbound and outbound traffic without creating a per-message
//! reply bridge.

use crate::agent::AgentWorker;
use crate::channels::feishu::FeishuChannel;
use crate::channels::{Channel, ChannelRegistration};
use crate::config::ChannelConfig;
use crate::model::{IncomingMessage, OutgoingMessage};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, info, warn};

pub struct ChannelRouter {
    agent: Arc<AgentWorker>,
    channels: RwLock<HashMap<String, mpsc::Sender<OutgoingMessage>>>,
    seen_messages: Mutex<HashSet<String>>,
}

impl ChannelRouter {
    /// Create a router around one agent worker instance.
    pub fn new(agent: AgentWorker) -> Arc<Self> {
        Arc::new(Self {
            agent: Arc::new(agent),
            channels: RwLock::new(HashMap::new()),
            seen_messages: Mutex::new(HashSet::new()),
        })
    }

    /// Build and register all configured channels.
    pub async fn register_channels(self: &Arc<Self>, config: &ChannelConfig) -> Result<()> {
        if !config.feishu_config().mode.trim().is_empty() {
            self.register_channel(Box::new(FeishuChannel::new(config.feishu_config().clone())))
                .await?;
        }

        Ok(())
    }

    /// Register a single channel, create its mpsc pair, and start its runtime loop.
    pub async fn register_channel(self: &Arc<Self>, channel: Box<dyn Channel>) -> Result<()> {
        let channel: Arc<dyn Channel> = Arc::from(channel);
        let health_channel = Arc::clone(&channel);
        let (incoming_tx, incoming_rx) = mpsc::channel(128);
        let (outgoing_tx, outgoing_rx) = mpsc::channel(128);

        channel.on_start().await?;
        self.channels
            .write()
            .await
            .insert(channel.name().to_string(), outgoing_tx);

        let router = Arc::clone(self);
        tokio::spawn(async move {
            router.run_incoming_loop(incoming_rx).await;
        });

        channel
            .start(ChannelRegistration {
                incoming_tx,
                outgoing_rx,
            })
            .await?;
        health_channel.check_health().await
    }

    /// Deduplicate and forward one normalized inbound message into the agent worker.
    pub async fn handle_incoming(self: &Arc<Self>, message: IncomingMessage) -> Result<()> {
        if !self
            .mark_message_seen(message.external_message_id.as_deref())
            .await
        {
            info!(
                external_message_id = ?message.external_message_id,
                "duplicate incoming message ignored"
            );
            return Ok(());
        }

        info!(
            channel = message.channel,
            user_id = message.user_id,
            "router accepted incoming message"
        );

        let (router_tx, mut router_rx) = mpsc::channel(128);
        let router = Arc::clone(self);
        let dispatch_task = tokio::spawn(async move {
            while let Some(outgoing) = router_rx.recv().await {
                if let Err(error) = router.dispatch_outgoing(outgoing).await {
                    warn!(error = %error, "router failed to dispatch outgoing message");
                }
            }
        });

        let result = self.agent.handle_message(message, router_tx).await;
        dispatch_task
            .await
            .map_err(|error| anyhow::anyhow!("router outgoing dispatch task failed: {}", error))?;
        result?;

        Ok(())
    }

    /// Dispatch one outbound message to the matching registered channel.
    pub async fn dispatch_outgoing(&self, message: OutgoingMessage) -> Result<()> {
        let channel_name = message.channel.clone();
        let channel_tx = self.channels
            .read()
            .await
            .get(&channel_name)
            .cloned();

        let Some(channel_tx) = channel_tx else {
            bail!("no registered channel found for `{channel_name}`");
        };

        debug!(
            channel = channel_name,
            "router dispatching outgoing message"
        );
        channel_tx
            .send(message)
            .await
            .map_err(|error| anyhow::anyhow!("failed to enqueue outgoing message: {}", error))
    }

    async fn mark_message_seen(&self, external_message_id: Option<&str>) -> bool {
        // Track upstream ids so the same external message is handled only once.
        let Some(external_message_id) = external_message_id else {
            return true;
        };

        let mut seen_messages = self.seen_messages.lock().await;
        seen_messages.insert(external_message_id.to_string())
    }

    async fn run_incoming_loop(self: Arc<Self>, mut incoming_rx: mpsc::Receiver<IncomingMessage>) {
        // Consume the channel-specific incoming queue serially for now.
        //
        // This keeps session mutation and agent execution easy to reason about in the current
        // in-memory design, but it also limits throughput because one slow turn blocks later
        // messages on the same channel loop. Future work should move to a long-lived agent inbox
        // and decouple inbound acceptance from turn execution.
        while let Some(message) = incoming_rx.recv().await {
            if let Err(error) = self.handle_incoming(message).await {
                warn!(error = %error, "router failed to process incoming message");
            }
        }
    }
}
