//! Message router that multiplexes channel traffic and agent traffic in one main loop.

use crate::agent::{
    AgentDispatchEvent, AgentRequest, AgentWorker, AgentWorkerEvent, AgentWorkerHandle,
    CommittedAgentDispatchItem,
};
use crate::attachment_syntax::AttachmentSyntaxParser;
use crate::channels::feishu::FeishuChannel;
use crate::channels::{Channel, ChannelRegistration};
use crate::command::{CommandRegistry, CommandReply};
use crate::config::ChannelConfig;
use crate::model::{IncomingMessage, OutgoingMessage};
use crate::session::{SessionManager, ThreadLocator};
use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Utc};
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
    feishu_deduper: FeishuMemoryDeduper,
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
}

impl Default for ChannelRouterBuilder {
    fn default() -> Self {
        Self {
            agent_handle: None,
            sessions: SessionManager::new(),
            commands: CommandRegistry::default(),
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
        self.sessions.install_thread_runtime(agent.thread_runtime());
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
    pub fn message_dedup_enabled(self, enabled: bool) -> Self {
        let _ = enabled;
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
            feishu_deduper: FeishuMemoryDeduper::default(),
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
    pub fn with_message_dedup_enabled(self, enabled: bool) -> Self {
        let _ = enabled;
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
        if !self.try_begin_channel_dedup(&message).await {
            return Ok(());
        }

        info!(
            channel = message.channel,
            user_id = message.user_id,
            "router accepted incoming message"
        );

        let locator = self.sessions.load_or_create_thread(&message).await?;

        if self.commands.is_command(&message)? {
            if self.is_thread_pending(&locator).await {
                if let Some(reply) = self.commands.running_thread_reply(&message)? {
                    info!(
                        thread_id = %locator.thread_id,
                        external_thread_id = %locator.external_thread_id,
                        "rejected idle-only command because thread is still running"
                    );
                    self.dispatch_command_reply(&message, reply).await?;
                    self.complete_channel_dedup(&message, true).await;
                    return Ok(());
                }
            }
            let mut thread_context = self
                .sessions
                .lock_thread_context(&locator, message.received_at)
                .await?;
            if let Some(reply) = self
                .commands
                .try_execute_with_thread_context(&message, &mut thread_context)
                .await?
            {
                self.dispatch_command_reply(&message, reply).await?;
                self.complete_channel_dedup(&message, true).await;
                return Ok(());
            }
        }

        if self.try_mark_thread_pending(&locator).await {
            if let Err(error) = self.dispatch_to_agent(locator, message.clone()).await {
                self.complete_channel_dedup(&message, false).await;
                return Err(error);
            }
        } else {
            self.enqueue_message(locator, message).await;
        }
        Ok(())
    }

    async fn handle_agent_event(&self, event: AgentWorkerEvent) -> Result<()> {
        match event {
            AgentWorkerEvent::DispatchItemCommitted(item) => {
                self.dispatch_committed_item(item).await
            }
            AgentWorkerEvent::RequestCompleted(request) => {
                self.complete_feishu_dedup_by_message_id(
                    request.external_message_id.as_deref(),
                    request.succeeded,
                )
                .await;
                self.release_or_dispatch_next(&request.locator).await
            }
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
            attachments: Vec::new(),
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
            attachments: Vec::new(),
            target: incoming.reply_target.clone(),
        })
        .await
    }

    /// Dispatch one outbound message to the matching registered channel.
    pub async fn dispatch_outgoing(&self, message: OutgoingMessage) -> Result<()> {
        let message = AttachmentSyntaxParser::parse_message(message);
        let channel_name = message.channel.clone();
        let Some(channel_tx) = self.channels.get(&channel_name) else {
            bail!("no registered channel found for `{channel_name}`");
        };
        debug!(
            channel = %channel_name,
            message_id = %message.id,
            attachment_count = message.attachments.len(),
            "router dispatching outgoing message"
        );
        channel_tx
            .send(message)
            .await
            .map_err(|error| anyhow::anyhow!("failed to enqueue outgoing message: {error}"))
    }

    async fn dispatch_committed_item(&self, committed: CommittedAgentDispatchItem) -> Result<()> {
        info!(
            thread_id = %committed.locator.thread_id,
            event_kind = ?committed.dispatch_event.kind,
            committed_at = %committed.committed_at,
            "router dispatching committed agent event"
        );
        self.process_agent_dispatch_event(committed.dispatch_event)
            .await
    }

    async fn try_begin_channel_dedup(&self, message: &IncomingMessage) -> bool {
        if message.channel != "feishu" {
            return true;
        }

        let Some(external_message_id) = message.external_message_id.as_deref() else {
            return true;
        };
        self.feishu_deduper.try_start(external_message_id).await
    }

    async fn complete_channel_dedup(&self, message: &IncomingMessage, succeeded: bool) {
        if message.channel != "feishu" {
            return;
        }
        self.complete_feishu_dedup_by_message_id(message.external_message_id.as_deref(), succeeded)
            .await;
    }

    async fn complete_feishu_dedup_by_message_id(
        &self,
        external_message_id: Option<&str>,
        succeeded: bool,
    ) {
        let Some(external_message_id) = external_message_id else {
            return;
        };
        if succeeded {
            self.feishu_deduper
                .mark_completed(external_message_id)
                .await;
        } else {
            self.feishu_deduper.clear_failed(external_message_id).await;
        }
    }

    async fn try_mark_thread_pending(&self, locator: &ThreadLocator) -> bool {
        let mut pending_threads = self.pending_threads.lock().await;
        pending_threads.insert(locator.clone())
    }

    async fn is_thread_pending(&self, locator: &ThreadLocator) -> bool {
        let pending_threads = self.pending_threads.lock().await;
        pending_threads.contains(locator)
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
        if let Err(error) = self
            .agent_tx
            .send(AgentRequest {
                locator: locator.clone(),
                incoming: message.clone(),
                sessions: self.sessions.clone(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum FeishuDedupStatus {
    Processing,
    Completed,
}

#[derive(Debug, Clone)]
struct FeishuDedupEntry {
    status: FeishuDedupStatus,
    expires_at: DateTime<Utc>,
}

#[derive(Debug)]
struct FeishuDedupState {
    entries: HashMap<String, FeishuDedupEntry>,
    last_cleanup_at: DateTime<Utc>,
}

#[derive(Debug)]
struct FeishuMemoryDeduper {
    state: Mutex<FeishuDedupState>,
    ttl: Duration,
    cleanup_interval: Duration,
}

impl Default for FeishuMemoryDeduper {
    fn default() -> Self {
        Self::new(Duration::minutes(10), Duration::minutes(1))
    }
}

impl FeishuMemoryDeduper {
    fn new(ttl: Duration, cleanup_interval: Duration) -> Self {
        let now = Utc::now();
        Self {
            state: Mutex::new(FeishuDedupState {
                entries: HashMap::new(),
                last_cleanup_at: now,
            }),
            ttl,
            cleanup_interval,
        }
    }

    async fn try_start(&self, external_message_id: &str) -> bool {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        self.cleanup_if_due(&mut state, now);
        match state.entries.get(external_message_id) {
            Some(entry) if entry.expires_at > now => {
                info!(
                    external_message_id,
                    status = ?entry.status,
                    expires_at = %entry.expires_at,
                    "feishu dedup hit existing entry"
                );
                false
            }
            _ => {
                state.entries.insert(
                    external_message_id.to_string(),
                    FeishuDedupEntry {
                        status: FeishuDedupStatus::Processing,
                        expires_at: now + self.ttl,
                    },
                );
                info!(
                    external_message_id,
                    expires_at = %(now + self.ttl),
                    "feishu dedup registered processing entry"
                );
                true
            }
        }
    }

    async fn mark_completed(&self, external_message_id: &str) {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        self.cleanup_if_due(&mut state, now);
        state.entries.insert(
            external_message_id.to_string(),
            FeishuDedupEntry {
                status: FeishuDedupStatus::Completed,
                expires_at: now + self.ttl,
            },
        );
        info!(
            external_message_id,
            expires_at = %(now + self.ttl),
            "feishu dedup marked completed"
        );
    }

    async fn clear_failed(&self, external_message_id: &str) {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        self.cleanup_if_due(&mut state, now);
        if state.entries.remove(external_message_id).is_some() {
            info!(
                external_message_id,
                "feishu dedup cleared failed processing entry"
            );
        }
    }

    fn cleanup_if_due(&self, state: &mut FeishuDedupState, now: DateTime<Utc>) {
        if now - state.last_cleanup_at < self.cleanup_interval {
            return;
        }
        let before = state.entries.len();
        state.entries.retain(|external_message_id, entry| {
            let keep = entry.expires_at > now;
            if !keep {
                info!(
                    external_message_id,
                    status = ?entry.status,
                    expired_at = %entry.expires_at,
                    "feishu dedup expired entry"
                );
            }
            keep
        });
        let removed = before.saturating_sub(state.entries.len());
        state.last_cleanup_at = now;
        if removed > 0 {
            info!(
                removed_entry_count = removed,
                remaining_entry_count = state.entries.len(),
                "feishu dedup cleanup removed expired entries"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FeishuMemoryDeduper;
    use chrono::Duration;

    #[tokio::test]
    async fn feishu_deduper_clears_failed_processing_entry_for_retry() {
        // 测试场景: Processing 请求失败后应删除 dedup 记录，允许平台重试重新进入主链路。
        let deduper = FeishuMemoryDeduper::default();

        assert!(deduper.try_start("msg_retry").await);
        assert!(!deduper.try_start("msg_retry").await);

        deduper.clear_failed("msg_retry").await;

        assert!(deduper.try_start("msg_retry").await);
    }

    #[tokio::test]
    async fn feishu_deduper_expires_completed_entry_after_ttl_cleanup() {
        // 测试场景: Completed 记录在 TTL 过期并清理后，应允许同一 external_message_id 再次进入。
        let deduper = FeishuMemoryDeduper::new(Duration::milliseconds(10), Duration::zero());

        assert!(deduper.try_start("msg_ttl").await);
        deduper.mark_completed("msg_ttl").await;
        assert!(!deduper.try_start("msg_ttl").await);

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        assert!(deduper.try_start("msg_ttl").await);
    }

    #[tokio::test]
    async fn feishu_deduper_is_process_local_best_effort_only() {
        // 测试场景: 进程重启后新的 deduper 实例不会继承旧记录，同一消息可能再次触发副作用。
        let first_process = FeishuMemoryDeduper::default();
        assert!(first_process.try_start("msg_restart").await);
        first_process.mark_completed("msg_restart").await;

        let restarted_process = FeishuMemoryDeduper::default();
        assert!(restarted_process.try_start("msg_restart").await);
    }
}
