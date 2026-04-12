//! Agent worker that owns the agent loop inbox and reports results back to the router.

use super::{
    agent_loop::{
        AgentCommittedMessageHandler, AgentDispatchEvent, AgentEventSender, AgentLoop,
        AgentLoopOutput,
    },
    runtime::AgentRuntime,
    sandbox::DummySandboxContainer,
};
use crate::compact::CompactProvider;
use crate::config::{
    AgentCompactConfig, AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, LLMConfig, global_config,
};
use crate::context::{ChatMessage, ChatMessageRole};
use crate::llm::{LLMProvider, build_provider, build_provider_from_global_config};
use crate::model::IncomingMessage;
use crate::session::{SessionManager, ThreadLocator};
use crate::thread::{Thread, ThreadRuntime};
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

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
    pub request_tx: mpsc::Sender<AgentRequest>,
    pub event_rx: mpsc::Receiver<AgentWorkerEvent>,
}

pub struct AgentWorker {
    agent_loop: AgentLoop,
    thread_runtime: Arc<ThreadRuntime>,
    sandbox: DummySandboxContainer,
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
/// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
/// use std::sync::Arc;
///
/// let worker = AgentWorker::builder()
///     .llm(Arc::new(MockLLMProvider::new("pong")))
///     .system_prompt("system")
///     .build()
///     .expect("worker should build");
///
/// assert!(worker.sandbox().is_placeholder());
/// ```
pub struct AgentWorkerBuilder {
    llm: Option<Arc<dyn LLMProvider>>,
    runtime: AgentRuntime,
    system_prompt: String,
    llm_config: LLMConfig,
    compact_config: AgentCompactConfig,
    compact_provider: Option<Arc<dyn CompactProvider>>,
}

impl Default for AgentWorkerBuilder {
    fn default() -> Self {
        Self {
            llm: None,
            runtime: AgentRuntime::new(),
            system_prompt: DEFAULT_ASSISTANT_SYSTEM_PROMPT.to_string(),
            llm_config: LLMConfig::default(),
            compact_config: AgentCompactConfig::default(),
            compact_provider: None,
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

    /// Override the system prompt injected into every turn.
    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = system_prompt.into();
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

    /// Build the worker from the accumulated fields.
    pub fn build(self) -> Result<AgentWorker> {
        let Self {
            llm,
            runtime,
            system_prompt,
            llm_config,
            compact_config,
            compact_provider,
        } = self;
        let Some(llm) = llm else {
            bail!("agent worker builder requires an llm provider");
        };
        let tool_registry = runtime.tools();
        let thread_runtime = Arc::new(ThreadRuntime::new(
            Arc::clone(&tool_registry),
            tool_registry.memory_repository(),
            system_prompt.clone(),
            compact_config.clone(),
        ));
        let agent_loop = match compact_provider {
            Some(compact_provider) => AgentLoop::with_compact_provider(
                llm,
                runtime,
                llm_config,
                compact_config.clone(),
                compact_provider,
            ),
            None => {
                AgentLoop::with_compact_config(llm, runtime, llm_config, compact_config.clone())
            }
        };

        Ok(AgentWorker {
            agent_loop,
            thread_runtime,
            sandbox: DummySandboxContainer::new(),
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
    /// ```rust
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let _worker = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// ```
    pub fn new(llm: Arc<dyn LLMProvider>, system_prompt: impl Into<String>) -> Self {
        Self::builder()
            .llm(llm)
            .system_prompt(system_prompt)
            .build()
            .expect("agent worker new should always have the required llm provider")
    }

    /// Create a worker with an explicitly provided runtime.
    pub fn with_runtime(
        llm: Arc<dyn LLMProvider>,
        system_prompt: impl Into<String>,
        runtime: AgentRuntime,
    ) -> Self {
        Self::builder()
            .llm(llm)
            .runtime(runtime)
            .system_prompt(system_prompt)
            .build()
            .expect("agent worker with_runtime should always have the required llm provider")
    }

    /// Create a worker with explicit runtime, LLM budget config, and compact config.
    pub fn with_runtime_and_compact_config(
        llm: Arc<dyn LLMProvider>,
        system_prompt: impl Into<String>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
    ) -> Self {
        Self::builder()
            .llm(llm)
            .runtime(runtime)
            .system_prompt(system_prompt)
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
    /// assert!(worker.sandbox().is_placeholder());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &AppConfig) -> Result<Self> {
        Self::builder()
            .llm(build_provider(config.llm_config())?)
            .runtime(AgentRuntime::from_config(config.agent_config()).await?)
            .system_prompt(config.llm_config().effective_system_prompt())
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
    /// assert!(worker.sandbox().is_placeholder());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_global_config() -> Result<Self> {
        let config = global_config();
        Self::builder()
            .llm(build_provider_from_global_config()?)
            .runtime(AgentRuntime::from_global_config().await?)
            .system_prompt(config.llm_config().effective_system_prompt())
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

    /// Return the sandbox container currently owned by this worker.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let worker = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// assert!(worker.sandbox().is_placeholder());
    /// ```
    pub fn sandbox(&self) -> &DummySandboxContainer {
        &self.sandbox
    }

    /// Spawn the long-lived agent worker loop and return its router-facing channels.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let worker = AgentWorker::builder()
    ///     .llm(Arc::new(MockLLMProvider::new("pong")))
    ///     .system_prompt("system")
    ///     .build()
    ///     .expect("worker should build");
    /// let handle = worker.spawn();
    /// let _ = handle.request_tx.clone();
    /// ```
    pub fn spawn(self) -> AgentWorkerHandle {
        let (request_tx, request_rx) = mpsc::channel(128);
        let (event_tx, event_rx) = mpsc::channel(128);

        tokio::spawn(async move {
            self.run(request_rx, event_tx).await;
        });

        AgentWorkerHandle {
            request_tx,
            event_rx,
        }
    }

    async fn run(
        self,
        mut request_rx: mpsc::Receiver<AgentRequest>,
        event_tx: mpsc::Sender<AgentWorkerEvent>,
    ) {
        while let Some(request) = request_rx.recv().await {
            if let Err(error) = self.handle_request(request, event_tx.clone()).await {
                warn!(
                    error = %format!("{error:#}"),
                    "agent worker failed to handle request"
                );
            }
        }
    }

    async fn handle_request(
        &self,
        request: AgentRequest,
        event_tx: mpsc::Sender<AgentWorkerEvent>,
    ) -> Result<AgentLoopOutput> {
        let mut thread_context = request
            .sessions
            .lock_thread_context(&request.locator, request.incoming.received_at)
            .await?;

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
            Ok(loop_output) => {
                event_tx
                    .send(AgentWorkerEvent::RequestCompleted(CompletedAgentRequest {
                        locator: request.locator.clone(),
                        completed_at: Utc::now(),
                        external_message_id: request.incoming.external_message_id.clone(),
                        succeeded: loop_output.succeeded,
                    }))
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("failed to report completed request: {error}")
                    })?;
                Ok(loop_output)
            }
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
                event_tx
                    .send(AgentWorkerEvent::RequestCompleted(CompletedAgentRequest {
                        locator: request.locator.clone(),
                        completed_at: Utc::now(),
                        external_message_id: request.incoming.external_message_id.clone(),
                        succeeded: false,
                    }))
                    .await
                    .map_err(|send_error| {
                        anyhow::anyhow!("failed to report completed fallback request: {send_error}")
                    })?;
                Err(error)
            }
        }
    }
}
