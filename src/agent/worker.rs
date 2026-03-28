//! Agent worker that owns the agent loop inbox and reports results back to the router.

use super::{
    agent_loop::{AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopOutput, InfoContext},
    runtime::AgentRuntime,
    sandbox::DummySandboxContainer,
};
use crate::compact::{CompactProvider, CompactScopeKey};
use crate::config::{AgentCompactConfig, AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, LLMConfig};
use crate::context::{ChatMessage, ChatMessageRole, MessageContext};
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
    pub thread: ConversationThread,
    pub history: Vec<ChatMessage>,
    pub loaded_toolsets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CompletedAgentTurn {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub thread_context: ThreadContext,
    pub active_thread: ConversationThread,
    pub messages: Vec<ChatMessage>,
    pub prepend_incoming_user: bool,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FailedAgentTurn {
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
    TurnCompleted(CompletedAgentTurn),
    TurnFailed(FailedAgentTurn),
}

pub struct AgentWorkerHandle {
    pub request_tx: mpsc::Sender<AgentRequest>,
    pub event_rx: mpsc::Receiver<AgentWorkerEvent>,
}

pub struct AgentWorker {
    agent_loop: AgentLoop,
    sandbox: DummySandboxContainer,
    system_prompt: String,
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
            Some(compact_provider) => AgentLoop::with_compact_provider(
                llm,
                self.runtime,
                self.llm_config,
                self.compact_config,
                compact_provider,
            ),
            None => AgentLoop::with_compact_config(
                llm,
                self.runtime,
                self.llm_config,
                self.compact_config,
            ),
        };

        Ok(AgentWorker {
            agent_loop,
            sandbox: DummySandboxContainer::new(),
            system_prompt: self.system_prompt,
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
        mut request: AgentRequest,
        event_tx: mpsc::Sender<AgentWorkerEvent>,
    ) -> Result<AgentLoopOutput> {
        let internal_thread_id = request.locator.thread_id.to_string();
        self.agent_loop
            .runtime()
            .tools()
            .merge_legacy_thread_state(&mut request.thread_context)
            .await;
        self.agent_loop
            .runtime()
            .compact_runtime()
            .merge_legacy_scope_overrides(
                &CompactScopeKey::from_locator(&request.locator),
                &mut request.thread_context,
            )
            .await;
        let context = build_context(
            &self.system_prompt,
            &request.thread_context,
            &request.incoming,
        );
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
            .run_with_thread_context(
                InfoContext {
                    channel: request.incoming.channel.clone(),
                    user_id: request.incoming.user_id.clone(),
                    thread_id: internal_thread_id.clone(),
                    compact_scope_key: CompactScopeKey::from_locator(&request.locator),
                    event_tx: AgentEventSender::new(
                        dispatch_tx,
                        request.incoming.channel.clone(),
                        request.incoming.external_thread_id.clone(),
                        request.incoming.external_message_id.clone(),
                        request.incoming.reply_target.clone(),
                        request.locator.session_id.to_string(),
                        request.locator.channel.clone(),
                        request.locator.user_id.clone(),
                        request.locator.external_thread_id.clone(),
                        request.locator.thread_id.to_string(),
                    ),
                },
                &context,
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
                    .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                        locator: request.locator,
                        incoming: request.incoming,
                        thread_context: loop_output.thread_context.clone(),
                        active_thread: loop_output.thread_context.to_conversation_thread(),
                        messages: loop_output.turn_messages.clone(),
                        prepend_incoming_user: loop_output.prepend_incoming_user,
                        loaded_toolsets: loop_output.loaded_toolsets.clone(),
                        tool_events: loop_output.tool_events.clone(),
                        completed_at,
                    }))
                    .await
                    .map_err(|error| anyhow::anyhow!("failed to report completed turn: {error}"))?;
                Ok(loop_output)
            }
            Err(error) => {
                let completed_at = Utc::now();
                let error_message = format!("{error:#}");
                event_tx
                    .send(AgentWorkerEvent::TurnFailed(FailedAgentTurn {
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
                        anyhow::anyhow!("failed to report failed turn: {send_error}")
                    })?;
                Err(error)
            }
        }
    }
}

fn build_context(
    system_prompt: &str,
    thread_context: &ThreadContext,
    incoming: &IncomingMessage,
) -> MessageContext {
    let mut context = MessageContext::with_system_prompt(system_prompt.to_string());
    context.extend_chat_messages(thread_context.load_messages());
    context.chat.push(ChatMessage::new(
        ChatMessageRole::User,
        incoming.content.clone(),
        incoming.received_at,
    ));
    context
}
