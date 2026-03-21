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
    pub fn new(agent: AgentWorker) -> Arc<Self> {
        // 作用: 创建 router，并持有 agent 与 channel 出站映射表。
        // 参数: agent 为 router 入站消息最终转发到的 agent worker。
        Arc::new(Self {
            agent: Arc::new(agent),
            channels: RwLock::new(HashMap::new()),
            seen_messages: Mutex::new(HashSet::new()),
        })
    }

    pub async fn register_channels(self: &Arc<Self>, config: &ChannelConfig) -> Result<()> {
        // 作用: 按配置批量构造并注册全部 channel。
        // 参数: config 为 channel 子配置，决定当前要启用哪些平台接入。
        if !config.feishu_config().mode.trim().is_empty() {
            self.register_channel(Box::new(FeishuChannel::new(config.feishu_config().clone())))
                .await?;
        }

        Ok(())
    }

    pub async fn register_channel(self: &Arc<Self>, channel: Box<dyn Channel>) -> Result<()> {
        // 作用: 为单个 channel 建立双向 mpsc 通道并启动其运行循环。
        // 参数: channel 为具体平台实现，负责实际收发消息。
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

    pub async fn handle_incoming(&self, message: IncomingMessage) -> Result<()> {
        // 作用: 接收 channel 转发来的统一入站消息，去重后交给 agent 执行。
        // 参数: message 为已经归一化的用户消息。
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

        if let Some(outgoing) = self.agent.handle_message(message).await? {
            self.dispatch_outgoing(outgoing).await?;
        }

        Ok(())
    }

    pub async fn dispatch_outgoing(&self, message: OutgoingMessage) -> Result<()> {
        // 作用: 按消息上的 channel 字段把出站消息派发给对应平台。
        // 参数: message 为 agent 生成的统一出站消息。
        let channel_name = message.channel.clone();
        let channel_tx = {
            let channels = self.channels.read().await;
            channels.get(&channel_name).cloned()
        };

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
        // 作用: 记录外部消息 ID，避免同一条消息被重复处理。
        // 参数: external_message_id 为 channel 平台返回的原始消息 ID。
        let Some(external_message_id) = external_message_id else {
            return true;
        };

        let mut seen_messages = self.seen_messages.lock().await;
        seen_messages.insert(external_message_id.to_string())
    }

    async fn run_incoming_loop(self: Arc<Self>, mut incoming_rx: mpsc::Receiver<IncomingMessage>) {
        // 作用: 持续消费 channel 入站消息队列，并串行交给 router 处理。
        // 参数: incoming_rx 为某个 channel 注册后的入站接收端。
        while let Some(message) = incoming_rx.recv().await {
            if let Err(error) = self.handle_incoming(message).await {
                warn!(error = %error, "router failed to process incoming message");
            }
        }
    }
}
