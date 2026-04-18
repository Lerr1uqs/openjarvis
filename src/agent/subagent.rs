//! Subagent runtime that reuses `AgentLoop`
//! behind one dedicated internal worker queue.

use super::{
    agent_loop::{AgentCommittedMessageHandler, AgentDispatchEvent, AgentEventSender, AgentLoop},
    hook::{HookEvent, HookEventKind},
    runtime::AgentRuntime,
};
use crate::{
    config::{AgentCompactConfig, LLMConfig},
    context::ChatMessage,
    llm::LLMProvider,
    model::{IncomingMessage, ReplyTarget},
    session::{SessionManager, ThreadLocator},
    thread::Thread,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SubagentRequest {
    pub parent_locator: ThreadLocator,
    pub child_locator: ThreadLocator,
    pub prompt: String,
    pub sessions: SessionManager,
}

impl SubagentRequest {
    fn subagent_key(&self) -> Option<&str> {
        self.child_locator
            .child_thread
            .as_ref()
            .map(|child| child.subagent_key.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct SubagentRunOutput {
    pub locator: ThreadLocator,
    pub output: super::AgentLoopOutput,
    pub dispatch_events: Vec<AgentDispatchEvent>,
}

struct QueuedSubagentRequest {
    request: SubagentRequest,
    response_tx: oneshot::Sender<Result<SubagentRunOutput>>,
}

struct SubagentCommittedMessageCollector {
    dispatch_events: Vec<AgentDispatchEvent>,
}

#[async_trait]
impl AgentCommittedMessageHandler for SubagentCommittedMessageCollector {
    async fn on_committed_message(
        &mut self,
        _thread_context: &mut Thread,
        _message: ChatMessage,
        dispatch_events: Vec<AgentDispatchEvent>,
    ) -> Result<()> {
        self.dispatch_events.extend(dispatch_events);
        Ok(())
    }
}

/// Dedicated runtime queue
/// used by subagent tool calls.
pub struct SubagentRunner {
    request_tx: mpsc::Sender<QueuedSubagentRequest>,
}

impl SubagentRunner {
    /// Build one dedicated subagent runner
    /// with its own internal worker queue.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{
    ///     agent::{AgentRuntime, SubagentRunner},
    ///     config::{AgentCompactConfig, LLMConfig},
    ///     llm::MockLLMProvider,
    /// };
    /// use std::sync::Arc;
    ///
    /// let _runner = SubagentRunner::new(
    ///     Arc::new(MockLLMProvider::new("subagent-reply")),
    ///     AgentRuntime::new(),
    ///     LLMConfig::default(),
    ///     AgentCompactConfig::default(),
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(
        llm: Arc<dyn LLMProvider>,
        runtime: AgentRuntime,
        llm_config: LLMConfig,
        compact_config: AgentCompactConfig,
    ) -> Self {
        let (request_tx, request_rx) = mpsc::channel(64);
        let agent_loop = AgentLoop::with_compact_config(llm, runtime, llm_config, compact_config);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                run_subagent_worker(agent_loop, request_rx).await;
            });
        } else {
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("subagent runner thread runtime should build");
                runtime.block_on(run_subagent_worker(agent_loop, request_rx));
            });
        }
        Self { request_tx }
    }

    /// Execute one subagent request synchronously and return the aggregated loop result.
    pub async fn run(&self, request: SubagentRequest) -> Result<SubagentRunOutput> {
        let (response_tx, response_rx) = oneshot::channel();
        self.request_tx
            .send(QueuedSubagentRequest {
                request,
                response_tx,
            })
            .await
            .map_err(|error| anyhow!("failed to enqueue subagent request: {error}"))?;
        response_rx
            .await
            .map_err(|error| anyhow!("subagent worker dropped response channel: {error}"))?
    }
}

impl std::fmt::Debug for SubagentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentRunner").finish_non_exhaustive()
    }
}

async fn run_subagent_worker(
    agent_loop: AgentLoop,
    mut request_rx: mpsc::Receiver<QueuedSubagentRequest>,
) {
    while let Some(queued) = request_rx.recv().await {
        let result = handle_subagent_request(&agent_loop, queued.request).await;
        if queued.response_tx.send(result).is_err() {
            warn!("subagent caller dropped response receiver before completion");
        }
    }
}

async fn handle_subagent_request(
    agent_loop: &AgentLoop,
    request: SubagentRequest,
) -> Result<SubagentRunOutput> {
    let hooks = agent_loop.runtime().hooks();
    let subagent_key = request.subagent_key().unwrap_or("unknown");
    let event_sender =
        AgentEventSender::for_subagent_thread(&request.parent_locator, &request.child_locator);
    info!(
        parent_thread_id = %request.parent_locator.thread_id,
        child_thread_id = %request.child_locator.thread_id,
        subagent_key = %subagent_key,
        "starting subagent worker request"
    );
    hooks
        .emit(HookEvent {
            kind: HookEventKind::SubagentStart,
            payload: json!({
                "parent_thread_id": request.parent_locator.thread_id.to_string(),
                "child_thread_id": request.child_locator.thread_id.to_string(),
                "subagent_key": subagent_key,
            }),
        })
        .await?;

    let now = Utc::now();
    let thread_context = request
        .sessions
        .lock_thread(&request.child_locator, now)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "child thread `{}` was not found before subagent execution",
                request.child_locator.thread_id
            )
        })?;
    let mut thread_context = thread_context;
    thread_context.bind_request_runtime(request.sessions.clone());

    let incoming = build_subagent_incoming(&request);
    let mut collector = SubagentCommittedMessageCollector {
        dispatch_events: Vec::new(),
    };
    let output = agent_loop
        .run_locked_thread(event_sender, &incoming, &mut thread_context, &mut collector)
        .await;
    let succeeded = output
        .as_ref()
        .map(|value| value.succeeded)
        .unwrap_or(false);
    hooks
        .emit(HookEvent {
            kind: HookEventKind::SubagentStop,
            payload: json!({
                "parent_thread_id": request.parent_locator.thread_id.to_string(),
                "child_thread_id": request.child_locator.thread_id.to_string(),
                "subagent_key": subagent_key,
                "succeeded": succeeded,
            }),
        })
        .await?;

    let output = output?;
    debug!(
        parent_thread_id = %request.parent_locator.thread_id,
        child_thread_id = %request.child_locator.thread_id,
        subagent_key = %subagent_key,
        dispatch_event_count = collector.dispatch_events.len(),
        "completed subagent worker request"
    );
    Ok(SubagentRunOutput {
        locator: request.child_locator,
        output,
        dispatch_events: collector.dispatch_events,
    })
}

fn build_subagent_incoming(request: &SubagentRequest) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: None,
        channel: request.child_locator.channel.clone(),
        user_id: request.child_locator.user_id.clone(),
        user_name: None,
        content: request.prompt.clone(),
        external_thread_id: Some(request.child_locator.external_thread_id.clone()),
        received_at: Utc::now(),
        metadata: json!({
            "internal_subagent": true,
            "parent_thread_id": request.parent_locator.thread_id.to_string(),
            "child_thread_id": request.child_locator.thread_id.to_string(),
            "subagent_key": request.subagent_key(),
        }),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: request.parent_locator.external_thread_id.clone(),
            receive_id_type: "internal_subagent".to_string(),
        },
    }
}
