//! Agent worker that owns the agent loop inbox and reports results back to the router.

use super::{
    agent_loop::{AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopOutput},
    runtime::AgentRuntime,
    sandbox::DummySandboxContainer,
};
use crate::compact::CompactProvider;
use crate::config::{AgentCompactConfig, AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, LLMConfig};
use crate::context::ChatMessage;
use crate::llm::{LLMProvider, build_provider};
use crate::model::IncomingMessage;
use crate::session::ThreadLocator;
use crate::thread::{ConversationThread, ThreadContext, ThreadToolEvent};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub thread_context: ThreadContext,
}

#[derive(Debug, Clone)]
pub struct CompletedAgentCommit {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub thread_context: ThreadContext,
    pub active_thread: ConversationThread,
    pub commit_messages: Vec<ChatMessage>,
    pub persist_incoming_user: bool,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FailedAgentCommit {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub error: String,
    pub thread_context: ThreadContext,
    pub active_thread: ConversationThread,
    pub loaded_toolsets: Vec<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum AgentWorkerEvent {
    Dispatch(AgentDispatchEvent),
    CommitCompleted(CompletedAgentCommit),
    CommitFailed(FailedAgentCommit),
}

pub struct AgentWorkerHandle {
    pub request_tx: mpsc::Sender<AgentRequest>,
    pub event_rx: mpsc::Receiver<AgentWorkerEvent>,
}

pub struct AgentWorker {
    agent_loop: AgentLoop,
    sandbox: DummySandboxContainer,
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
        let Some(llm) = self.llm else {
            bail!("agent worker builder requires an llm provider");
        };
        let agent_loop = match self.compact_provider {
            Some(compact_provider) => AgentLoop::with_compact_provider_and_system_prompt(
                llm,
                self.runtime,
                self.llm_config,
                self.compact_config,
                compact_provider,
                Some(self.system_prompt.clone()),
            ),
            None => AgentLoop::with_compact_config_and_system_prompt(
                llm,
                self.runtime,
                self.llm_config,
                self.compact_config,
                Some(self.system_prompt.clone()),
            ),
        };

        Ok(AgentWorker {
            agent_loop,
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
            .system_prompt(DEFAULT_ASSISTANT_SYSTEM_PROMPT)
            .llm_config(config.llm_config().clone())
            .compact_config(config.agent_config().compact_config().clone())
            .build()
    }

    /// Return the runtime bound to this worker.
    pub fn runtime(&self) -> &AgentRuntime {
        self.agent_loop.runtime()
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
        let (dispatch_tx, mut dispatch_rx) = mpsc::channel(128);
        let forward_event_tx = event_tx.clone();
        let forward_dispatch_task = tokio::spawn(async move {
            while let Some(dispatch_event) = dispatch_rx.recv().await {
                if forward_event_tx
                    .send(AgentWorkerEvent::Dispatch(dispatch_event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let loop_output = self
            .agent_loop
            .run_v1(
                AgentEventSender::from_incoming_and_locator(
                    dispatch_tx,
                    &request.incoming,
                    &request.thread_context.locator,
                ),
                &request.incoming,
                request.thread_context.clone(),
            )
            .await;
        forward_dispatch_task
            .await
            .map_err(|error| anyhow::anyhow!("agent dispatch forward task failed: {error}"))?;

        match loop_output {
            Ok(loop_output) => {
                let completed_at = Utc::now();
                event_tx // TODO: 需要重构
                    .send(AgentWorkerEvent::CommitCompleted(CompletedAgentCommit {
                        locator: request.locator,
                        incoming: request.incoming,
                        thread_context: loop_output.thread_context.clone(),
                        active_thread: loop_output.thread_context.to_conversation_thread(),
                        commit_messages: loop_output.commit_messages.clone(),
                        persist_incoming_user: loop_output.persist_incoming_user,
                        loaded_toolsets: loop_output.loaded_toolsets.clone(),
                        tool_events: loop_output.tool_events.clone(),
                        completed_at,
                    }))
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("failed to report completed commit: {error}")
                    })?;
                Ok(loop_output)
            }
            Err(error) => {
                let completed_at = Utc::now();
                let error_message = format!("{error:#}");
                event_tx
                    .send(AgentWorkerEvent::CommitFailed(FailedAgentCommit {
                        locator: request.locator,
                        incoming: request.incoming,
                        error: error_message.clone(),
                        thread_context: request.thread_context.clone(),
                        active_thread: request.thread_context.to_conversation_thread(),
                        loaded_toolsets: request.thread_context.load_toolsets(),
                        completed_at,
                    }))
                    .await
                    .map_err(|send_error| {
                        anyhow::anyhow!("failed to report failed commit: {send_error}")
                    })?;
                Err(error)
            }
        }
    }
}
