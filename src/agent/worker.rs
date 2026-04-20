//! Agent worker that owns the agent loop inbox and reports results back to the router.

use super::{
    agent_loop::{
        AgentCommittedMessageHandler, AgentDispatchEvent, AgentEventSender, AgentLoop,
        AgentLoopOutput,
    },
    runtime::AgentRuntime,
    sandbox::{Sandbox, SandboxCapabilityConfig, build_sandbox},
    subagent::SubagentRunner,
};
use crate::compact::CompactProvider;
use crate::config::{AgentCompactConfig, AppConfig, LLMConfig, global_config};
use crate::context::{ChatMessage, ChatMessageRole};
use crate::llm::{LLMProvider, build_provider, build_provider_from_global_config};
use crate::model::IncomingMessage;
use crate::queue::{
    ClaimedTopicQueueMessage, TopicQueue, TopicQueueLeaseAcquireResult, TopicQueueWorkerLease,
};
use crate::session::{SessionManager, ThreadLocator};
use crate::thread::{Thread, ThreadRuntime};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::{collections::HashSet, pin::Pin, sync::Arc, time::Duration as StdDuration};
use tokio::{
    sync::{Mutex, mpsc},
    time::{Instant, Sleep},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub sessions: SessionManager,
}

#[derive(Debug, Clone)]
pub struct CommittedAgentDispatchItem {
    pub locator: ThreadLocator,
    pub dispatch_event: AgentDispatchEvent,
    pub committed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CompletedAgentRequest {
    pub locator: ThreadLocator,
    pub completed_at: DateTime<Utc>,
    pub external_message_id: Option<String>,
    pub succeeded: bool,
}

#[derive(Debug, Clone)]
pub enum AgentWorkerEvent {
    DispatchItemCommitted(CommittedAgentDispatchItem),
    RequestCompleted(CompletedAgentRequest),
}

pub struct AgentWorkerHandle {
    controller: AgentWorkerHandleController,
    event_rx: Option<mpsc::Receiver<AgentWorkerEvent>>,
}

pub struct AgentWorker {
    agent_loop: AgentLoop,
    thread_runtime: Arc<ThreadRuntime>,
    subagent_runner: Arc<SubagentRunner>,
    sandbox: Arc<dyn Sandbox>,
}

enum AgentWorkerHandleController {
    DomainPool(Arc<AgentDomainWorkerPool>),
    Noop,
}

struct AgentDomainWorkerPool {
    worker: Arc<AgentWorker>,
    queue: Arc<dyn TopicQueue>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    live_domains: Arc<Mutex<HashSet<String>>>,
}

struct WorkerCommittedMessageHandler {
    locator: ThreadLocator,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
}

#[async_trait]
impl AgentCommittedMessageHandler for WorkerCommittedMessageHandler {
    async fn on_committed_message(
        &mut self,
        _thread_context: &mut Thread,
        message: ChatMessage,
        dispatch_events: Vec<AgentDispatchEvent>,
    ) -> Result<()> {
        for dispatch_event in dispatch_events {
            self.event_tx
                .send(AgentWorkerEvent::DispatchItemCommitted(
                    CommittedAgentDispatchItem {
                        locator: self.locator.clone(),
                        committed_at: message.created_at,
                        dispatch_event,
                    },
                ))
                .await
                .map_err(|error| {
                    anyhow::anyhow!("failed to report committed dispatch item: {error}")
                })?;
        }
        Ok(())
    }
}

/// Builder for assembling one [`AgentWorker`] with explicit runtime and compact settings.
///
/// # 示例
/// ```rust
/// use openjarvis::{agent::{AgentWorker, SandboxCapabilityConfig}, llm::MockLLMProvider};
/// use std::sync::Arc;
///
/// let sandbox_capabilities = SandboxCapabilityConfig::from_yaml_str(
///     "sandbox:\n  backend: disabled\n",
///     ".",
/// )
/// .expect("sandbox capability config should parse");
/// let worker = AgentWorker::builder()
///     .llm(Arc::new(MockLLMProvider::new("pong")))
///     .sandbox_capabilities(sandbox_capabilities)
///     .build()
///     .expect("worker should build");
///
/// assert_eq!(worker.sandbox().kind(), "disabled");
/// ```
pub struct AgentWorkerBuilder {
    llm: Option<Arc<dyn LLMProvider>>,
    runtime: AgentRuntime,
    llm_config: LLMConfig,
    compact_config: AgentCompactConfig,
    compact_provider: Option<Arc<dyn CompactProvider>>,
    sandbox_capabilities: Option<SandboxCapabilityConfig>,
}

impl Default for AgentWorkerBuilder {
    fn default() -> Self {
        Self {
            llm: None,
            runtime: AgentRuntime::new(),
            llm_config: LLMConfig::default(),
            compact_config: AgentCompactConfig::default(),
            compact_provider: None,
            sandbox_capabilities: None,
        }
    }
}

impl AgentWorkerBuilder {
    /// Create one empty worker builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the LLM provider used by the worker.
    pub fn llm(mut self, llm: Arc<dyn LLMProvider>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Replace the runtime container used by the worker.
    pub fn runtime(mut self, runtime: AgentRuntime) -> Self {
        self.runtime = runtime;
        self
    }

    /// Replace the LLM budget config used by compact estimation.
    pub fn llm_config(mut self, llm_config: LLMConfig) -> Self {
        self.llm_config = llm_config;
        self
    }

    /// Replace the compact config used by runtime compact and auto-compact.
    pub fn compact_config(mut self, compact_config: AgentCompactConfig) -> Self {
        self.compact_config = compact_config;
        self
    }

    /// Override the compact provider used by runtime compaction.
    pub fn compact_provider(mut self, compact_provider: Arc<dyn CompactProvider>) -> Self {
        self.compact_provider = Some(compact_provider);
        self
    }

    /// Override the sandbox capability policy used by the worker.
    pub fn sandbox_capabilities(mut self, sandbox_capabilities: SandboxCapabilityConfig) -> Self {
        self.sandbox_capabilities = Some(sandbox_capabilities);
        self
    }

    /// Build the worker from the accumulated fields.
    pub fn build(self) -> Result<AgentWorker> {
        let Self {
            llm,
            runtime,
            llm_config,
            compact_config,
            compact_provider,
            sandbox_capabilities,
        } = self;
        let Some(llm) = llm else {
            bail!("agent worker builder requires an llm provider");
        };
        let tool_registry = runtime.tools();
        let thread_runtime = Arc::new(ThreadRuntime::new(
            Arc::clone(&tool_registry),
            tool_registry.memory_repository(),
            compact_config.clone(),
        ));
        let agent_loop = match compact_provider {
            Some(compact_provider) => AgentLoop::with_compact_provider(
                Arc::clone(&llm),
                runtime.clone(),
                llm_config.clone(),
                compact_config.clone(),
                compact_provider,
            ),
            None => AgentLoop::with_compact_config(
                Arc::clone(&llm),
                runtime.clone(),
                llm_config.clone(),
                compact_config.clone(),
            ),
        };
        let subagent_runner = Arc::new(SubagentRunner::new(
            llm,
            runtime.clone(),
            llm_config,
            compact_config,
        ));
        tool_registry.install_subagent_runner(&subagent_runner);
        let sandbox_capabilities = match sandbox_capabilities {
            Some(config) => config,
            None => SandboxCapabilityConfig::load_for_workspace(std::env::current_dir().context(
                "failed to resolve current workspace root for sandbox capability config",
            )?)?,
        };
        let sandbox = build_sandbox(sandbox_capabilities)?;
        tool_registry.install_sandbox(Arc::clone(&sandbox));

        Ok(AgentWorker {
            agent_loop,
            thread_runtime,
            subagent_runner,
            sandbox,
        })
    }
}

impl AgentWorker {
    /// Create one worker builder.
    pub fn builder() -> AgentWorkerBuilder {
        AgentWorkerBuilder::new()
    }

    /// Create a worker with a fresh default runtime.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let _worker = AgentWorker::new(Arc::new(MockLLMProvider::new("pong")));
    /// ```
    pub fn new(llm: Arc<dyn LLMProvider>) -> Self {
        Self::builder()
            .llm(llm)
            .build()
            .expect("agent worker new should always have the required llm provider")
    }

    /// Create a worker with an explicitly provided runtime.
    pub fn with_runtime(llm: Arc<dyn LLMProvider>, runtime: AgentRuntime) -> Self {
        Self::builder()
            .llm(llm)
            .runtime(runtime)
            .build()
            .expect("agent worker with_runtime should always have the required llm provider")
    }

    /// Create a worker with explicit runtime, LLM budget config, and compact config.
    pub fn with_runtime_and_compact_config(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
    ) -> Self {
        Self::builder()
            .llm(llm)
            .runtime(runtime)
            .llm_config(llm_config)
            .compact_config(compact_config)
            .build()
            .expect("agent worker with_runtime_and_compact_config should always have the required llm provider")
    }

    /// Build a worker directly from the loaded app configuration.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::AgentWorker, config::AppConfig};
    ///
    /// let worker = AgentWorker::from_config(&AppConfig::default()).await?;
    /// assert_eq!(worker.sandbox().kind(), "disabled");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &AppConfig) -> Result<Self> {
        Self::builder()
            .llm(build_provider(config.llm_config())?)
            .runtime(AgentRuntime::from_config(config.agent_config()).await?)
            .llm_config(config.llm_config().clone())
            .compact_config(config.agent_config().compact_config().clone())
            .build()
    }

    /// Build a worker directly from the installed global app config snapshot.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{
    ///     agent::AgentWorker,
    ///     config::{AppConfig, install_global_config},
    /// };
    ///
    /// let config = AppConfig::builder_for_test().build()?;
    /// install_global_config(config)?;
    ///
    /// let worker = AgentWorker::from_global_config().await?;
    /// assert_eq!(worker.sandbox().kind(), "disabled");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_global_config() -> Result<Self> {
        let config = global_config();
        Self::builder()
            .llm(build_provider_from_global_config()?)
            .runtime(AgentRuntime::from_global_config().await?)
            .llm_config(config.llm_config().clone())
            .compact_config(config.agent_config().compact_config().clone())
            .build()
    }

    /// Return the runtime bound to this worker.
    pub fn runtime(&self) -> &AgentRuntime {
        self.agent_loop.runtime()
    }

    /// Return the thread runtime used to initialize and hydrate threads.
    pub fn thread_runtime(&self) -> Arc<ThreadRuntime> {
        Arc::clone(&self.thread_runtime)
    }

    /// Return the dedicated subagent runner bound to this worker runtime.
    pub fn subagent_runner(&self) -> Arc<SubagentRunner> {
        Arc::clone(&self.subagent_runner)
    }

    /// Return the sandbox container currently owned by this worker.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{agent::{AgentWorker, SandboxCapabilityConfig}, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let sandbox_capabilities = SandboxCapabilityConfig::from_yaml_str(
    ///     "sandbox:\n  backend: disabled\n",
    ///     ".",
    /// )
    /// .expect("sandbox capability config should parse");
    /// let worker = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .sandbox_capabilities(sandbox_capabilities)
    ///     .build()
    ///     .expect("worker should build");
    /// assert_eq!(worker.sandbox().kind(), "disabled");
    /// ```
    pub fn sandbox(&self) -> &(dyn Sandbox + '_) {
        self.sandbox.as_ref()
    }

    /// Spawn the domain worker pool and return its router-facing event channel.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::AgentWorker,
    ///     llm::MockLLMProvider,
    ///     queue::{PostgresTopicQueue, TopicQueue, TopicQueueRuntimeConfig},
    /// };
    /// use std::sync::Arc;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let worker = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .build()
    ///     .expect("worker should build");
    /// let queue: Arc<dyn TopicQueue> = Arc::new(
    ///     PostgresTopicQueue::connect(
    ///         "postgres://postgres:postgres@127.0.0.1:5432/openjarvis",
    ///         TopicQueueRuntimeConfig::default(),
    ///     )
    ///     .await?,
    /// );
    /// let mut handle = worker.spawn(queue);
    /// let _ = handle.event_rx_mut()?.try_recv();
    /// # Ok(())
    /// # }
    /// ```
    pub fn spawn(self, queue: Arc<dyn TopicQueue>) -> AgentWorkerHandle {
        let (event_tx, event_rx) = mpsc::channel(128);
        let pool = Arc::new(AgentDomainWorkerPool {
            worker: Arc::new(self),
            queue,
            event_tx,
            live_domains: Arc::new(Mutex::new(HashSet::new())),
        });
        AgentWorkerHandle {
            controller: AgentWorkerHandleController::DomainPool(pool),
            event_rx: Some(event_rx),
        }
    }

    async fn handle_request(
        &self,
        request: AgentRequest,
        event_tx: mpsc::Sender<AgentWorkerEvent>,
    ) -> Result<AgentLoopOutput> {
        let thread_context = request
            .sessions
            .lock_thread(&request.locator, request.incoming.received_at)
            .await?;
        let mut thread_context = thread_context.ok_or_else(|| {
            anyhow::anyhow!(
                "thread `{}` was not found before worker handling",
                request.locator.thread_id
            )
        })?;
        thread_context.bind_request_runtime(request.sessions.clone());

        let mut committed_message_handler = WorkerCommittedMessageHandler {
            locator: request.locator.clone(),
            event_tx: event_tx.clone(),
        };
        let loop_output = self
            .agent_loop
            .run_locked_thread(
                AgentEventSender::from_incoming_and_locator(
                    &request.incoming,
                    &thread_context.locator,
                ),
                &request.incoming,
                &mut thread_context,
                &mut committed_message_handler,
            )
            .await;

        match loop_output {
            Ok(loop_output) => Ok(loop_output),
            Err(error) => {
                warn!(
                    error = %format!("{error:#}"),
                    thread_id = %request.locator.thread_id,
                    "agent loop returned one hard failure, attempting thread-owned fallback"
                );
                if !thread_context.has_active_request() {
                    thread_context.begin_request(
                        request.incoming.external_message_id.clone(),
                        request.incoming.received_at,
                    )?;
                }
                let committed_at = Utc::now();
                let failure_reply = format!("[openjarvis][agent_error] {error:#}");
                let failure_message = ChatMessage::new(
                    ChatMessageRole::Assistant,
                    failure_reply.clone(),
                    committed_at,
                );
                thread_context.push_message(failure_message.clone()).await?;
                event_tx
                    .send(AgentWorkerEvent::DispatchItemCommitted(
                        CommittedAgentDispatchItem {
                            locator: request.locator.clone(),
                            dispatch_event: AgentEventSender::from_incoming_and_locator(
                                &request.incoming,
                                &thread_context.locator,
                            )
                            .prepare_dispatch_event(
                                super::agent_loop::AgentLoopEvent {
                                    kind: super::agent_loop::AgentLoopEventKind::TextOutput,
                                    content: failure_reply.clone(),
                                    metadata: serde_json::json!({
                                        "source": "worker_fallback_failure",
                                        "is_final": true,
                                        "is_error": true,
                                    }),
                                },
                                request.incoming.external_message_id.clone(),
                                true,
                            ),
                            committed_at,
                        },
                    ))
                    .await
                    .map_err(|send_error| {
                        anyhow::anyhow!(
                            "failed to report fallback committed dispatch item: {send_error}"
                        )
                    })?;
                thread_context.finish_request(Utc::now(), false)?;
                Err(error)
            }
        }
    }
}

impl AgentWorkerHandle {
    /// Construct one no-op handle for tests that only need the downstream event channel.
    pub fn noop(event_rx: mpsc::Receiver<AgentWorkerEvent>) -> Self {
        Self {
            controller: AgentWorkerHandleController::Noop,
            event_rx: Some(event_rx),
        }
    }

    /// Borrow the downstream event receiver for direct event assertions in tests and callers.
    pub fn event_rx_mut(&mut self) -> Result<&mut mpsc::Receiver<AgentWorkerEvent>> {
        self.event_rx
            .as_mut()
            .context("agent worker handle event receiver has already been taken")
    }

    /// Transfer the downstream event receiver into the router main loop exactly once.
    pub(crate) fn take_event_rx(&mut self) -> Result<mpsc::Receiver<AgentWorkerEvent>> {
        self.event_rx
            .take()
            .context("agent worker handle event receiver has already been taken")
    }

    /// Ensure one domain worker task exists for the provided thread domain.
    pub async fn ensure_worker(
        &self,
        domain: impl Into<String>,
        sessions: SessionManager,
    ) -> Result<bool> {
        match &self.controller {
            AgentWorkerHandleController::DomainPool(pool) => {
                pool.ensure_worker(domain.into(), sessions).await
            }
            AgentWorkerHandleController::Noop => Ok(false),
        }
    }
}

impl AgentDomainWorkerPool {
    async fn ensure_worker(
        self: &Arc<Self>,
        domain: String,
        sessions: SessionManager,
    ) -> Result<bool> {
        let domain = domain.trim().to_string();
        if domain.is_empty() {
            bail!("agent worker domain must not be blank");
        }

        {
            let mut live_domains = self.live_domains.lock().await;
            if !live_domains.insert(domain.clone()) {
                debug!(domain = %domain, "skip worker spawn because local domain task already exists");
                return Ok(false);
            }
        }

        let pool = Arc::clone(self);
        let domain_for_task = domain.clone();
        tokio::spawn(async move {
            let join_result = tokio::spawn({
                let pool = Arc::clone(&pool);
                let domain_for_join = domain_for_task.clone();
                async move { pool.run_domain_worker_task(domain_for_join, sessions).await }
            })
            .await;
            match join_result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(domain = %domain_for_task, error = %format!("{error:#}"), "domain worker task exited with error");
                }
                Err(error) => {
                    warn!(domain = %domain_for_task, error = %error, "domain worker task panicked");
                }
            }
            pool.live_domains.lock().await.remove(&domain);
        });
        Ok(true)
    }
    async fn run_domain_worker_task(
        self: Arc<Self>,
        domain: String,
        sessions: SessionManager,
    ) -> Result<()> {
        let worker_id = Uuid::new_v4().to_string();
        let lease = match self
            .queue
            .acquire_worker_lease(&domain, &worker_id, Utc::now())
            .await?
        {
            TopicQueueLeaseAcquireResult::Acquired(lease) => lease,
            TopicQueueLeaseAcquireResult::Busy => {
                debug!(
                    domain = %domain,
                    worker_id,
                    "skip domain worker start because another lease is already active"
                );
                return Ok(());
            }
        };

        let result = Arc::clone(&self)
            .run_domain_worker_loop(lease.clone(), sessions)
            .await;
        if let Err(error) = self.queue.release_worker(&lease, Utc::now()).await {
            warn!(
                domain = %lease.domain,
                worker_id = %lease.worker_id,
                error = %format!("{error:#}"),
                "failed to release domain worker lease on exit"
            );
        }
        result
    }

    async fn run_domain_worker_loop(
        self: Arc<Self>,
        lease: TopicQueueWorkerLease,
        sessions: SessionManager,
    ) -> Result<()> {
        let runtime_config = self.queue.runtime_config();
        let idle_timeout = runtime_config.idle_timeout;
        let heartbeat_interval = runtime_config.heartbeat_interval;
        let poll_interval = worker_poll_interval(heartbeat_interval, idle_timeout);
        let mut idle_deadline = Instant::now() + idle_timeout;

        info!(
            domain = %lease.domain,
            worker_id = %lease.worker_id,
            "started domain queue worker"
        );

        loop {
            let now = Utc::now();
            if let Some(claimed) = self.queue.claim(&lease.domain, &lease, now).await? {
                Arc::clone(&self)
                    .process_claimed_message(
                        lease.clone(),
                        claimed,
                        sessions.clone(),
                        heartbeat_interval,
                    )
                    .await?;
                idle_deadline = Instant::now() + idle_timeout;
                continue;
            }

            if Instant::now() >= idle_deadline {
                info!(
                    domain = %lease.domain,
                    worker_id = %lease.worker_id,
                    "domain queue worker exited after idle timeout"
                );
                return Ok(());
            }

            tokio::time::sleep(poll_interval).await;
            let heartbeat_ok = self.queue.heartbeat_worker(&lease, Utc::now()).await?;
            if !heartbeat_ok {
                warn!(
                    domain = %lease.domain,
                    worker_id = %lease.worker_id,
                    "domain queue worker lease is no longer active during idle heartbeat"
                );
                return Ok(());
            }
        }
    }

    async fn process_claimed_message(
        self: Arc<Self>,
        lease: TopicQueueWorkerLease,
        claimed: ClaimedTopicQueueMessage,
        sessions: SessionManager,
        heartbeat_interval: StdDuration,
    ) -> Result<()> {
        let request = AgentRequest {
            locator: claimed.payload.locator.clone(),
            incoming: claimed.payload.incoming.clone(),
            sessions,
        };
        let locator = request.locator.clone();
        let external_message_id = request.incoming.external_message_id.clone();
        let event_tx = self.event_tx.clone();
        let processing = async {
            let succeeded = match self.worker.handle_request(request, event_tx.clone()).await {
                Ok(loop_output) => loop_output.succeeded,
                Err(error) => {
                    warn!(
                        domain = %lease.domain,
                        worker_id = %lease.worker_id,
                        message_id = %claimed.message_id,
                        error = %format!("{error:#}"),
                        "domain worker finished one claimed message through fallback failure path"
                    );
                    false
                }
            };
            if !self
                .queue
                .complete(claimed.message_id, &claimed.claim_token, Utc::now())
                .await?
            {
                bail!(
                    "queue complete rejected claimed message `{}` on domain `{}`",
                    claimed.message_id,
                    lease.domain
                );
            }
            event_tx
                .send(AgentWorkerEvent::RequestCompleted(CompletedAgentRequest {
                    locator,
                    completed_at: Utc::now(),
                    external_message_id,
                    succeeded,
                }))
                .await
                .map_err(|error| {
                    anyhow::anyhow!("failed to report completed queue message: {error}")
                })?;
            Ok(())
        };
        tokio::pin!(processing);
        let mut heartbeat_sleep = heartbeat_sleep(heartbeat_interval);
        loop {
            tokio::select! {
                result = &mut processing => return result,
                _ = &mut heartbeat_sleep => {
                    let heartbeat_ok = self.queue.heartbeat_worker(&lease, Utc::now()).await?;
                    if !heartbeat_ok {
                        warn!(
                            domain = %lease.domain,
                            worker_id = %lease.worker_id,
                            message_id = %claimed.message_id,
                            "domain worker lost active lease while processing; queue remains at-least-once"
                        );
                    }
                    heartbeat_sleep.as_mut().reset(Instant::now() + heartbeat_interval);
                }
            }
        }
    }
}

fn worker_poll_interval(heartbeat_interval: StdDuration, idle_timeout: StdDuration) -> StdDuration {
    let candidate = std::cmp::min(heartbeat_interval, idle_timeout) / 4;
    if candidate.is_zero() {
        StdDuration::from_millis(100)
    } else {
        std::cmp::min(candidate, StdDuration::from_secs(1))
    }
}

fn heartbeat_sleep(heartbeat_interval: StdDuration) -> Pin<Box<Sleep>> {
    Box::pin(tokio::time::sleep(heartbeat_interval))
}
