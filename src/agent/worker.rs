//! Agent worker that stitches sessions, context construction, and the agent loop together.
//!
//! At the moment the worker is invoked directly by the router and processes one message at a
//! time from that call path. This keeps ordering straightforward while the system still uses an
//! in-memory session store. A later refactor should move the worker behind a long-lived inbox so
//! router and agent communicate through stable channels, and then concurrency can be introduced
//! in a controlled way without breaking per-session ordering.

use super::{
    agent_loop::{AgentEventSender, AgentLoop, AgentLoopOutput, InfoContext},
    runtime::AgentRuntime,
};
use crate::config::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, LlmConfig};
use crate::llm::{LLMProvider, build_provider};
use crate::model::IncomingMessage;
use crate::session::SessionManager;
use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AgentWorker {
    agent_loop: AgentLoop,
    system_prompt: String,
    sessions: SessionManager,
}

impl AgentWorker {
    /// Create a worker with a fresh default runtime and session store.
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
            sessions: SessionManager::new(),
        }
    }

    /// Build a worker directly from the loaded LLM configuration.
    pub fn from_config(config: &LlmConfig) -> Result<Self> {
        Ok(Self::new(
            build_provider(config)?,
            DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        ))
    }

    /// Return the runtime bound to this worker.
    pub fn runtime(&self) -> &AgentRuntime {
        self.agent_loop.runtime()
    }

    /// Process one normalized incoming message through session assembly and the agent loop.
    ///
    /// The current version assumes turns arrive in execution order from the caller. When the
    /// agent later owns its own inbox, this method should become the inner step of that loop
    /// rather than the public concurrency boundary.
    pub async fn handle_message(
        &self,
        incoming: IncomingMessage,
        router_tx: mpsc::Sender<crate::model::OutgoingMessage>,
    ) -> Result<AgentLoopOutput> {
        let pending_turn = self // TODO: begin or create session turn
            .sessions
            .begin_turn(&incoming, &self.system_prompt)
            .await;
        let loop_output = self
            .agent_loop
            .run(
                InfoContext {
                    channel: incoming.channel.clone(),
                    user_id: incoming.user_id.clone(),
                    thread_id: pending_turn.thread_id.clone(),
                    event_tx: AgentEventSender::new(
                        router_tx,
                        incoming.channel.clone(),
                        pending_turn.thread_id.clone(),
                        incoming.external_message_id.clone(),
                        incoming.reply_target.clone(),
                        pending_turn.session_key.channel.clone(),
                        pending_turn.session_key.user_id.clone(),
                        pending_turn.thread_id.clone(),
                    ),
                },
                &pending_turn.context,
            )
            .await?;
        self.sessions// TODO: fillback
            .complete_turn_with_messages(
                &pending_turn,
                loop_output.turn_messages.clone(),
                Utc::now(),
            )
            .await;

        Ok(loop_output)
    }
}
