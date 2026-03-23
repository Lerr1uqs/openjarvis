//! Message router that multiplexes channel traffic and agent traffic in one main loop.

use crate::agent::{
    AgentDispatchEvent, AgentRequest, AgentWorker, AgentWorkerEvent, AgentWorkerHandle,
    CompletedAgentTurn, FailedAgentTurn,
};
use crate::channels::feishu::FeishuChannel;
use crate::channels::{Channel, ChannelRegistration};
use crate::config::ChannelConfig;
use crate::context::{ChatMessage, ChatMessageRole};
use crate::model::{IncomingMessage, OutgoingMessage};
use crate::session::{SessionManager, ThreadLocator};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::{StreamExt, StreamMap, wrappers::ReceiverStream};
use tracing::{debug, info, warn};
use uuid::Uuid;

pub struct ChannelRouter {
    agent_tx: mpsc::Sender<AgentRequest>,
    agent_event_rx: mpsc::Receiver<AgentWorkerEvent>,
    channel_incoming_streams: StreamMap<String, ReceiverStream<IncomingMessage>>,
    channels: HashMap<String, mpsc::Sender<OutgoingMessage>>,
    sessions: SessionManager,
    seen_messages: Mutex<HashSet<String>>,
    pending_threads: Mutex<HashSet<ThreadLocator>>,
    queued_messages: Mutex<HashMap<ThreadLocator, VecDeque<IncomingMessage>>>,
}

impl ChannelRouter {
    /// Create a router around one long-lived agent worker.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
    /// use std::sync::Arc;
    ///
    /// let agent = AgentWorker::new(Arc::new(MockLLMProvider::new("pong")), "system");
    /// let _router = ChannelRouter::new(agent);
    /// ```
    pub fn new(agent: AgentWorker) -> Self {
        Self::with_session_manager(agent, SessionManager::new())
    }

    /// Create a router with an explicit session manager instance.
    pub fn with_session_manager(agent: AgentWorker, sessions: SessionManager) -> Self {
        Self::with_session_manager_and_agent_handle(agent.spawn(), sessions)
    }

    /// Create a router around an already constructed agent handle.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::AgentWorkerHandle,
    ///     router::ChannelRouter,
    ///     session::SessionManager,
    /// };
    /// use tokio::sync::mpsc;
    ///
    /// let (request_tx, _request_rx) = mpsc::channel(8);
    /// let (_event_tx, event_rx) = mpsc::channel(8);
    /// let handle = AgentWorkerHandle { request_tx, event_rx };
    /// let _router = ChannelRouter::with_session_manager_and_agent_handle(
    ///     handle,
    ///     SessionManager::new(),
    /// );
    /// ```
    pub fn with_session_manager_and_agent_handle(
        agent_handle: AgentWorkerHandle,
        sessions: SessionManager,
    ) -> Self {
        Self {
            agent_tx: agent_handle.request_tx,
            agent_event_rx: agent_handle.event_rx,
            channel_incoming_streams: StreamMap::new(),
            channels: HashMap::new(),
            sessions,
            seen_messages: Mutex::new(HashSet::new()),
            pending_threads: Mutex::new(HashSet::new()),
            queued_messages: Mutex::new(HashMap::new()),
        }
    }

    /// Return the session manager owned by the router.
    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    /// Build and register all configured channels.
    pub async fn register_channels(&mut self, config: &ChannelConfig) -> Result<()> {
        if !config.feishu_config().mode.trim().is_empty() {
            self.register_channel(Box::new(FeishuChannel::new(config.feishu_config().clone())))
                .await?;
        }

        Ok(())
    }

    /// Register a single channel, create its mpsc pair, and start its runtime loop.
    pub async fn register_channel(&mut self, channel: Box<dyn Channel>) -> Result<()> {
        let channel: Arc<dyn Channel> = Arc::from(channel);
        let health_channel = Arc::clone(&channel);
        let (incoming_tx, incoming_rx) = mpsc::channel(128);
        let (outgoing_tx, outgoing_rx) = mpsc::channel(128);

        channel.on_start().await?;
        self.channels
            .insert(channel.name().to_string(), outgoing_tx);
        self.channel_incoming_streams
            .insert(channel.name().to_string(), ReceiverStream::new(incoming_rx));

        channel
            .start(ChannelRegistration {
                incoming_tx,
                outgoing_rx,
            })
            .await?;
        health_channel.check_health().await
    }

    /// Run the main router loop and multiplex channel and agent traffic with `tokio::select!`.
    pub async fn run(&mut self) -> Result<()> {
        loop {
            let channel_incoming_streams = &mut self.channel_incoming_streams;
            let agent_event_rx = &mut self.agent_event_rx;

            tokio::select! {
                Some((_channel_name, message)) = channel_incoming_streams.next(), if !channel_incoming_streams.is_empty() => {
                    if let Err(error) = self.handle_incoming(message).await {
                        warn!(error = %error, "router failed to process incoming message");
                    };
                }
                Some(agent_event) = agent_event_rx.recv() => {
                    if let Err(error) = self.handle_agent_event(agent_event).await {
                        warn!(error = %error, "router failed to process agent event");
                    };
                }
                else => break, // TODO: add signal ctrl-c?
            }
        }

        Ok(())
    }

    async fn handle_incoming(&self, message: IncomingMessage) -> Result<()> {
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

        let locator = self.sessions.load_or_create_thread(&message).await;
        if self.try_mark_thread_pending(&locator).await {
            self.dispatch_to_agent(locator, message).await?;
        } else {
            self.enqueue_message(locator, message).await;
        }
        Ok(())
    }

    async fn handle_agent_event(&self, event: AgentWorkerEvent) -> Result<()> {
        match event {
            AgentWorkerEvent::Dispatch(event) => self.process_agent_dispatch_event(event).await,
            AgentWorkerEvent::TurnCompleted(turn) => self.store_completed_turn(turn).await,
            AgentWorkerEvent::TurnFailed(turn) => self.store_failed_turn(turn).await,
        }
    }

    async fn process_agent_dispatch_event(&self, event: AgentDispatchEvent) -> Result<()> {
        let source_message_id = event.source_message_id.clone();
        let outgoing = OutgoingMessage {
            id: Uuid::new_v4(),
            channel: event.channel,
            content: event.content,
            thread_id: event.thread_id,
            metadata: serde_json::json!({
                "source_message_id": source_message_id,
                "session_id": event.session_id,
                "session_channel": event.session_channel,
                "session_user_id": event.session_user_id,
                "session_external_thread_id": event.session_external_thread_id,
                "session_thread_id": event.session_thread_id,
                "event_kind": format!("{:?}", event.kind),
                "event_metadata": event.metadata,
            }),
            reply_to_message_id: if event.reply_to_source {
                event.source_message_id
            } else {
                None
            },
            target: event.target,
        };
        self.dispatch_outgoing(outgoing).await
    }

    /// Dispatch one outbound message to the matching registered channel.
    pub async fn dispatch_outgoing(&self, message: OutgoingMessage) -> Result<()> {
        let channel_name = message.channel.clone();
        let Some(channel_tx) = self.channels.get(&channel_name) else {
            bail!("no registered channel found for `{channel_name}`");
        };

        debug!(
            channel = channel_name,
            "router dispatching outgoing message"
        );
        channel_tx
            .send(message)
            .await
            .map_err(|error| anyhow::anyhow!("failed to enqueue outgoing message: {error}"))
    }

    async fn store_completed_turn(&self, turn: CompletedAgentTurn) -> Result<()> {
        let mut messages = vec![ChatMessage::new(
            ChatMessageRole::User,
            turn.incoming.content.clone(),
            turn.incoming.received_at,
        )];
        messages.extend(turn.messages);
        self.sessions
            .store_turn(
                &turn.locator,
                turn.incoming.external_message_id.clone(),
                messages,
                turn.incoming.received_at,
                turn.completed_at,
            )
            .await;
        self.release_or_dispatch_next(&turn.locator).await?;
        Ok(())
    }

    async fn store_failed_turn(&self, turn: FailedAgentTurn) -> Result<()> {
        let failure_reply = format!("[openjarvis][agent_error] {}", turn.error);
        self.dispatch_outgoing(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: turn.incoming.channel.clone(),
            content: failure_reply.clone(),
            thread_id: turn.incoming.thread_id.clone(),
            metadata: serde_json::json!({
                "event_kind": "AgentError",
                "session_id": turn.locator.session_id.to_string(),
                "error": turn.error,
                "session_channel": turn.locator.channel,
                "session_user_id": turn.locator.user_id,
                "session_external_thread_id": turn.locator.external_thread_id,
                "session_thread_id": turn.locator.thread_id.to_string(),
            }),
            reply_to_message_id: turn.incoming.external_message_id.clone(),
            target: turn.incoming.reply_target.clone(),
        })
        .await?;

        self.sessions
            .store_turn(
                &turn.locator,
                turn.incoming.external_message_id.clone(),
                vec![
                    ChatMessage::new(
                        ChatMessageRole::User,
                        turn.incoming.content,
                        turn.incoming.received_at,
                    ),
                    ChatMessage::new(ChatMessageRole::Assistant, failure_reply, turn.completed_at),
                ],
                turn.incoming.received_at,
                turn.completed_at,
            )
            .await;
        self.release_or_dispatch_next(&turn.locator).await?;
        Ok(())
    }

    async fn mark_message_seen(&self, external_message_id: Option<&str>) -> bool {
        let Some(external_message_id) = external_message_id else {
            return true;
        };

        let mut seen_messages = self.seen_messages.lock().await;
        seen_messages.insert(external_message_id.to_string())
    }

    async fn try_mark_thread_pending(&self, locator: &ThreadLocator) -> bool {
        let mut pending_threads = self.pending_threads.lock().await;
        pending_threads.insert(locator.clone())
    }

    async fn enqueue_message(&self, locator: ThreadLocator, message: IncomingMessage) {
        let mut queued_messages = self.queued_messages.lock().await;
        queued_messages
            .entry(locator)
            .or_default()
            .push_back(message);
    }

    async fn dispatch_to_agent(
        &self,
        locator: ThreadLocator,
        message: IncomingMessage,
    ) -> Result<()> {
        let history = self.sessions.load_turn(&locator).await;
        if let Err(error) = self
            .agent_tx
            .send(AgentRequest {
                locator: locator.clone(),
                incoming: message.clone(),
                history,
            })
            .await
        {
            self.pending_threads.lock().await.remove(&locator);
            return Err(anyhow::anyhow!("failed to enqueue agent request: {error}"));
        }
        Ok(())
    }

    async fn release_or_dispatch_next(&self, locator: &ThreadLocator) -> Result<()> {
        let next_message = {
            let queued_messages = self.queued_messages.lock().await;
            queued_messages
                .get(locator)
                .and_then(|queue| queue.front().cloned())
        };

        if let Some(message) = next_message {
            self.dispatch_to_agent(locator.clone(), message).await?;
            let mut queued_messages = self.queued_messages.lock().await;
            if let Some(queue) = queued_messages.get_mut(locator) {
                queue.pop_front();
                if queue.is_empty() {
                    queued_messages.remove(locator);
                }
            }
            return Ok(());
        }

        self.pending_threads.lock().await.remove(locator);
        Ok(())
    }
}
