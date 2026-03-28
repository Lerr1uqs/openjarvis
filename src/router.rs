//! Message router that multiplexes channel traffic and agent traffic in one main loop.

use crate::agent::{
    AgentDispatchEvent, AgentRequest, AgentWorker, AgentWorkerEvent, AgentWorkerHandle,
    CompletedAgentTurn, FailedAgentTurn,
};
use crate::channels::feishu::FeishuChannel;
use crate::channels::{Channel, ChannelRegistration};
use crate::command::{CommandRegistry, CommandReply};
use crate::config::ChannelConfig;
use crate::context::{ChatMessage, ChatMessageRole};
use crate::model::{IncomingMessage, OutgoingMessage};
use crate::session::{SessionManager, ThreadLocator};
use crate::thread::ThreadContext;
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::{Future, pending};
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
    commands: CommandRegistry,
    message_dedup_enabled: bool,
    seen_messages: Mutex<HashSet<String>>,
    pending_threads: Mutex<HashSet<ThreadLocator>>,
    queued_messages: Mutex<HashMap<ThreadLocator, VecDeque<IncomingMessage>>>,
}

/// Builder for assembling one [`ChannelRouter`] around an agent worker or handle.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
/// use std::sync::Arc;
///
/// let agent = AgentWorker::builder()
///     .llm(Arc::new(MockLLMProvider::new("pong")))
///     .system_prompt("system")
///     .build()
///     .expect("worker should build");
/// let router = ChannelRouter::builder()
///     .agent(agent)
///     .build()
///     .expect("router should build");
///
/// let _ = router.sessions();
/// ```
pub struct ChannelRouterBuilder {
    agent_handle: Option<AgentWorkerHandle>,
    sessions: SessionManager,
    commands: CommandRegistry,
    message_dedup_enabled: bool,
}

impl Default for ChannelRouterBuilder {
    fn default() -> Self {
        Self {
            agent_handle: None,
            sessions: SessionManager::new(),
            commands: CommandRegistry::default(),
            message_dedup_enabled: false,
        }
    }
}

impl ChannelRouterBuilder {
    /// Create one empty router builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach one long-lived agent worker and spawn its runtime handle.
    pub fn agent(mut self, agent: AgentWorker) -> Self {
        self.agent_handle = Some(agent.spawn());
        self
    }

    /// Attach one already constructed agent handle.
    pub fn agent_handle(mut self, agent_handle: AgentWorkerHandle) -> Self {
        self.agent_handle = Some(agent_handle);
        self
    }

    /// Replace the session manager used by the router.
    pub fn session_manager(mut self, sessions: SessionManager) -> Self {
        self.sessions = sessions;
        self
    }

    /// Replace the command registry used by the router.
    pub fn command_registry(mut self, commands: CommandRegistry) -> Self {
        self.commands = commands;
        self
    }

    /// Enable or disable router-level message deduplication.
    pub fn message_dedup_enabled(mut self, enabled: bool) -> Self {
        self.message_dedup_enabled = enabled;
        self
    }

    /// Build the router from the accumulated fields.
    pub fn build(self) -> Result<ChannelRouter> {
        let Some(agent_handle) = self.agent_handle else {
            bail!("channel router builder requires an agent worker or agent handle");
        };

        Ok(ChannelRouter {
            agent_tx: agent_handle.request_tx,
            agent_event_rx: agent_handle.event_rx,
            channel_incoming_streams: StreamMap::new(),
            channels: HashMap::new(),
            sessions: self.sessions,
            commands: self.commands,
            message_dedup_enabled: self.message_dedup_enabled,
            seen_messages: Mutex::new(HashSet::new()),
            pending_threads: Mutex::new(HashSet::new()),
            queued_messages: Mutex::new(HashMap::new()),
        })
    }
}

impl ChannelRouter {
    /// Create one router builder.
    pub fn builder() -> ChannelRouterBuilder {
        ChannelRouterBuilder::new()
    }

    /// Create a router around one long-lived agent worker.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
    /// use std::sync::Arc;
    ///
    /// let agent = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let _router = ChannelRouter::builder()
    ///     .agent(agent)
    ///     .build()
    ///     .expect("router should build");
    /// ```
    pub fn new(agent: AgentWorker) -> Self {
        Self::builder()
            .agent(agent)
            .build()
            .expect("channel router new should always have the required agent worker")
    }

    /// Create a router with an explicit session manager instance.
    pub fn with_session_manager(agent: AgentWorker, sessions: SessionManager) -> Self {
        Self::builder()
            .agent(agent)
            .session_manager(sessions)
            .build()
            .expect(
                "channel router with_session_manager should always have the required agent worker",
            )
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
    /// let _router = ChannelRouter::builder()
    ///     .agent_handle(handle)
    ///     .session_manager(SessionManager::new())
    ///     .build()
    ///     .expect("router should build");
    /// ```
    pub fn with_session_manager_and_agent_handle(
        agent_handle: AgentWorkerHandle,
        sessions: SessionManager,
    ) -> Self {
        Self::builder()
            .agent_handle(agent_handle)
            .session_manager(sessions)
            .build()
            .expect("channel router with_session_manager_and_agent_handle should always have the required agent handle")
    }

    /// Return the session manager owned by the router.
    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    /// Replace the router's command registry.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::AgentWorker,
    ///     command::CommandRegistry,
    ///     llm::MockLLMProvider,
    ///     router::ChannelRouter,
    /// };
    /// use std::sync::Arc;
    ///
    /// let agent = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let _router = ChannelRouter::builder()
    ///     .agent(agent)
    ///     .command_registry(CommandRegistry::default())
    ///     .build()
    ///     .expect("router should build");
    /// ```
    pub fn with_command_registry(mut self, commands: CommandRegistry) -> Self {
        self.commands = commands;
        self
    }

    /// Enable or disable external-message deduplication.
    ///
    /// Deduplication is currently disabled by default so command framework work can proceed
    /// without the old router-level filtering semantics.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
    /// use std::sync::Arc;
    ///
    /// let agent = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let _router = ChannelRouter::builder()
    ///     .agent(agent)
    ///     .message_dedup_enabled(true)
    ///     .build()
    ///     .expect("router should build");
    /// ```
    pub fn with_message_dedup_enabled(mut self, enabled: bool) -> Self {
        self.message_dedup_enabled = enabled;
        self
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

    /// Run the router as a long-lived service loop.
    ///
    /// This variant keeps waiting for new external inputs until the router encounters an error.
    /// Process-level shutdown should use [`Self::run_until_shutdown`] instead of relying on this
    /// loop to finish on its own.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
    /// use std::sync::Arc;
    ///
    /// let agent = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let mut router = ChannelRouter::builder()
    ///     .agent(agent)
    ///     .build()
    ///     .expect("router should build");
    /// tokio::spawn(async move {
    ///     let _ = router.run().await;
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&mut self) -> Result<()> {
        self.run_until_shutdown(pending::<()>()).await
    }

    /// Run the router until a caller-provided shutdown signal resolves.
    ///
    /// This is intended for process lifecycle control and tests. The router still exits early
    /// when its downstream agent worker channel disconnects.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider, router::ChannelRouter};
    /// use std::sync::Arc;
    /// use tokio::sync::oneshot;
    ///
    /// let agent = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let mut router = ChannelRouter::builder()
    ///     .agent(agent)
    ///     .build()
    ///     .expect("router should build");
    /// let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    /// router
    ///     .run_until_shutdown(async move {
    ///         let _ = shutdown_rx.await;
    ///     })
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_until_shutdown<F>(&mut self, shutdown: F) -> Result<()>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);

        loop {
            let channel_incoming_streams = &mut self.channel_incoming_streams;
            let agent_event_rx = &mut self.agent_event_rx;

            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                incoming = channel_incoming_streams.next(), if !channel_incoming_streams.is_empty() => {
                    if let Some((_channel_name, message)) = incoming {
                        if let Err(error) = self.handle_incoming(message).await {
                            warn!(error = %error, "router failed to process incoming message");
                        };
                    }
                }
                agent_event = agent_event_rx.recv() => {
                    // TODO: 用match会不会更好?
                    if let Some(agent_event) = agent_event {
                        if let Err(error) = self.handle_agent_event(agent_event).await {
                            warn!(error = %error, "router failed to process agent event");
                        };
                    } else {
                        // TODO: if the downstream agent worker crashes, router should rebuild or
                        // restart the worker instead of exiting the service loop.
                        bail!("agent worker event channel closed")
                    }
                }
                else => return Ok(()),
            }
        }
    }

    async fn handle_incoming(&self, message: IncomingMessage) -> Result<()> {
        if self.message_dedup_enabled
            && !self
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

        let locator = self.sessions.load_or_create_thread(&message).await?;
        if let Some(external_message_id) = message.external_message_id.as_deref()
            && self
                .sessions
                .is_external_message_processed(&locator, external_message_id)
                .await?
        {
            info!(
                thread_id = %locator.thread_id,
                external_message_id,
                "duplicate incoming message ignored by persisted dedup record"
            );
            return Ok(());
        }

        if self.commands.is_command(&message)? {
            let mut thread_context = self
                .sessions
                .load_thread_context(&locator)
                .await
                ?
                .unwrap_or_else(|| ThreadContext::new((&locator).into(), message.received_at));
            if let Some(reply) = self
                .commands
                .try_execute_with_thread_context(&message, &mut thread_context)
                .await?
            {
                self.sessions
                    .store_thread_context(&locator, thread_context, message.received_at)
                    .await?;
                self.dispatch_command_reply(&message, reply).await?;
                if let Some(external_message_id) = message.external_message_id.as_deref() {
                    self.sessions
                        .mark_external_message_processed(
                            &locator,
                            external_message_id,
                            None,
                            message.received_at,
                        )
                        .await?;
                }
                return Ok(());
            }
        }

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
            external_thread_id: event.external_thread_id,
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

    async fn dispatch_command_reply(
        &self,
        incoming: &IncomingMessage,
        reply: CommandReply,
    ) -> Result<()> {
        self.dispatch_outgoing(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: incoming.channel.clone(),
            content: reply.formatted_content(),
            external_thread_id: incoming.external_thread_id.clone(),
            metadata: serde_json::json!({
                "event_kind": "Command",
                "command_name": reply.name(),
                "command_status": if reply.is_success() { "SUCCESS" } else { "FAILED" },
            }),
            reply_to_message_id: incoming.external_message_id.clone(),
            target: incoming.reply_target.clone(),
        })
        .await
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
        let mut messages = Vec::new();
        if turn.prepend_incoming_user {
            messages.push(ChatMessage::new(
                ChatMessageRole::User,
                turn.incoming.content.clone(),
                turn.incoming.received_at,
            ));
        }
        messages.extend(turn.messages);
        let store_result = self
            .sessions
            .store_turn_with_thread_context(
                &turn.locator,
                Some(turn.thread_context),
                turn.incoming.external_message_id.clone(),
                messages,
                turn.incoming.received_at,
                turn.completed_at,
            )
            .await;
        let release_result = self.release_or_dispatch_next(&turn.locator).await;
        store_result?;
        release_result
    }

    async fn store_failed_turn(&self, turn: FailedAgentTurn) -> Result<()> {
        let failure_reply = format!("[openjarvis][agent_error] {}", turn.error);
        self.dispatch_outgoing(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: turn.incoming.channel.clone(),
            content: failure_reply.clone(),
            external_thread_id: turn.incoming.external_thread_id.clone(),
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

        let store_result = self
            .sessions
            .store_turn_with_thread_context(
                &turn.locator,
                Some(turn.thread_context),
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
        let release_result = self.release_or_dispatch_next(&turn.locator).await;
        store_result?;
        release_result
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
        let thread_state = self.sessions.load_thread_state(&locator).await?;
        let thread_context = thread_state
            .thread_context
            .unwrap_or_else(|| ThreadContext::new((&locator).into(), message.received_at));
        if let Err(error) = self
            .agent_tx
            .send(AgentRequest {
                locator: locator.clone(),
                incoming: message.clone(),
                thread: thread_context.to_conversation_thread(),
                history: thread_context.load_messages(),
                loaded_toolsets: thread_context.load_toolsets(),
                thread_context,
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
