//! Agent worker that owns the agent loop inbox and reports results back to the router.

use super::{
    agent_loop::{AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopOutput, InfoContext},
    runtime::AgentRuntime,
};
use crate::config::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, LLMConfig};
use crate::context::{ChatMessage, ChatMessageRole, MessageContext};
use crate::llm::{LLMProvider, build_provider};
use crate::model::IncomingMessage;
use crate::session::ThreadLocator;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub history: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct CompletedAgentTurn {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub messages: Vec<ChatMessage>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FailedAgentTurn {
    pub locator: ThreadLocator,
    pub incoming: IncomingMessage,
    pub error: String,
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
    system_prompt: String,
}

impl AgentWorker {
    /// Create a worker with a fresh default runtime.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let _worker = AgentWorker::new(Arc::new(MockLLMProvider::new("pong")), "system");
    /// ```
    pub fn new(llm: Arc<dyn LLMProvider>, system_prompt: impl Into<String>) -> Self {
        Self::with_runtime(llm, system_prompt, AgentRuntime::new())
    }

    /// Create a worker with an explicitly provided runtime.
    pub fn with_runtime(
        llm: Arc<dyn LLMProvider>,
        system_prompt: impl Into<String>,
        runtime: AgentRuntime,
    ) -> Self {
        Self {
            agent_loop: AgentLoop::new(llm, runtime),
            system_prompt: system_prompt.into(),
        }
    }

    /// Build a worker directly from the loaded LLM configuration.
    pub fn from_config(config: &LLMConfig) -> Result<Self> {
        Ok(Self::new(
            build_provider(config)?,
            DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        ))
    }

    /// Return the runtime bound to this worker.
    pub fn runtime(&self) -> &AgentRuntime {
        self.agent_loop.runtime()
    }

    /// Spawn the long-lived agent worker loop and return its router-facing channels.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{agent::AgentWorker, llm::MockLLMProvider};
    /// use std::sync::Arc;
    ///
    /// let worker = AgentWorker::new(Arc::new(MockLLMProvider::new("pong")), "system");
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
                warn!(error = %error, "agent worker failed to handle request");
            }
        }
    }

    async fn handle_request(
        &self,
        request: AgentRequest,
        event_tx: mpsc::Sender<AgentWorkerEvent>,
    ) -> Result<AgentLoopOutput> {
        let context = build_context(&self.system_prompt, &request.history, &request.incoming);
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
            .run(
                InfoContext {
                    channel: request.incoming.channel.clone(),
                    user_id: request.incoming.user_id.clone(),
                    thread_id: request.locator.external_thread_id.clone(),
                    event_tx: AgentEventSender::new(
                        dispatch_tx,
                        request.incoming.channel.clone(),
                        request.incoming.thread_id.clone(),
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
            )
            .await;
        forward_dispatch_task
            .await
            .map_err(|error| anyhow::anyhow!("agent dispatch forward task failed: {error}"))?;

        match loop_output {
            Ok(loop_output) => {
                let completed_at = Utc::now();
                event_tx
                    .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                        locator: request.locator,
                        incoming: request.incoming,
                        messages: loop_output.turn_messages.clone(),
                        completed_at,
                    }))
                    .await
                    .map_err(|error| anyhow::anyhow!("failed to report completed turn: {error}"))?;
                Ok(loop_output)
            }
            Err(error) => {
                let completed_at = Utc::now();
                let error_message = error.to_string();
                event_tx
                    .send(AgentWorkerEvent::TurnFailed(FailedAgentTurn {
                        locator: request.locator,
                        incoming: request.incoming,
                        error: error_message.clone(),
                        completed_at,
                    }))
                    .await
                    .map_err(|send_error| {
                        anyhow::anyhow!("failed to report failed turn: {send_error}")
                    })?;
                Err(anyhow::anyhow!(error_message))
            }
        }
    }
}

fn build_context(
    system_prompt: &str,
    history: &[ChatMessage],
    incoming: &IncomingMessage,
) -> MessageContext {
    let mut context = MessageContext::with_system_prompt(system_prompt.to_string());
    context.extend_chat_messages(history.iter().cloned());
    context.chat.push(ChatMessage::new(
        ChatMessageRole::User,
        incoming.content.clone(),
        incoming.received_at,
    ));
    context
}
