use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentRequest, AgentWorker, AgentWorkerBuilder, AgentWorkerEvent},
    config::AppConfig,
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::SessionManager,
    thread::{ConversationThread, ThreadContext, ThreadContextLocator},
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};
use uuid::Uuid;

#[tokio::test]
async fn worker_spawn_emits_outgoing_and_completed_turn() {
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("mock-reply")))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = SessionManager::new().load_or_create_thread(&incoming).await;

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            thread_context: ThreadContext::from_conversation_thread(
                ThreadContextLocator::from(&locator),
                ConversationThread::with_id(locator.thread_id, "default", incoming.received_at),
            ),
            thread: ConversationThread::with_id(locator.thread_id, "default", incoming.received_at),
            history: Vec::new(),
            loaded_toolsets: Vec::new(),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 2).await;

    match &events[0] {
        AgentWorkerEvent::Dispatch(event) => {
            assert_eq!(event.content, "mock-reply");
            assert_eq!(event.external_thread_id, None);
            assert_eq!(event.session_external_thread_id, "default");
            assert_eq!(format!("{:?}", event.kind), "TextOutput");
            assert!(event.reply_to_source);
            assert_eq!(event.source_message_id.as_deref(), Some("msg_1"));
        }
        other => panic!("unexpected first event: {other:?}"),
    }

    match &events[1] {
        AgentWorkerEvent::TurnCompleted(turn) => {
            assert_eq!(turn.locator.thread_id, locator.thread_id);
            assert_eq!(turn.messages.len(), 1);
            assert_eq!(turn.messages[0].content, "mock-reply");
        }
        other => panic!("unexpected second event: {other:?}"),
    }
}

#[test]
fn worker_holds_dummy_sandbox_container() {
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("mock-reply")))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");

    assert_eq!(worker.sandbox().kind(), "dummy");
    assert!(worker.sandbox().is_placeholder());
}

#[test]
fn worker_builder_requires_llm_provider() {
    // 测试场景: builder 缺少 LLM provider 时必须报错，避免构造出无效 worker。
    let error = AgentWorkerBuilder::new()
        .system_prompt("system prompt")
        .build()
        .err()
        .expect("builder without llm should fail");

    assert!(error.to_string().contains("llm provider"));
}

struct RecordingProvider {
    requests: Arc<Mutex<Vec<LLMRequest>>>,
}

#[async_trait]
impl LLMProvider for RecordingProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        self.requests.lock().await.push(request);
        Ok(LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "ok",
                Utc::now(),
            )),
            tool_calls: Vec::new(),
        })
    }
}

struct FailingProvider;

#[async_trait]
impl LLMProvider for FailingProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        Err(anyhow::anyhow!("provider said 429: rate limit exceeded")).context(
            "failed to call llm provider `openai_compatible` model `demo-model` at `https://provider.test/v1`",
        )
    }
}

#[tokio::test]
async fn worker_builds_context_from_history_and_current_user_message() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let worker = AgentWorker::builder()
        .llm(Arc::new(RecordingProvider {
            requests: Arc::clone(&requests),
        }))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("what happened");
    let locator = SessionManager::new().load_or_create_thread(&incoming).await;
    let thread = thread_with_history(
        locator.thread_id,
        &[ChatMessage::new(
            ChatMessageRole::Assistant,
            "previous reply",
            Utc::now(),
        )],
    );

    handle
        .request_tx
        .send(AgentRequest {
            thread_context: ThreadContext::from_conversation_thread(
                ThreadContextLocator::from(&locator),
                thread.clone(),
            ),
            locator,
            incoming,
            thread,
            history: vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                "previous reply",
                Utc::now(),
            )],
            loaded_toolsets: Vec::new(),
        })
        .await
        .expect("request should be accepted");

    let _ = collect_events(handle.event_rx, 2).await;
    let captured_requests = requests.lock().await;
    let messages = &captured_requests[0].messages;

    assert_eq!(messages[0].content, "system prompt");
    assert!(
        messages
            .iter()
            .any(|message| message.content == "previous reply")
    );
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("what happened")
    );
}

#[tokio::test]
async fn worker_from_config_loads_configured_hooks() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  hook:
    notification: ["echo", "hook loaded"]
llm:
  provider: "mock"
"#,
    )
    .expect("config should parse");
    let worker = AgentWorker::from_config(&config)
        .await
        .expect("worker should build from config");

    assert_eq!(worker.runtime().hooks().len().await, 1);
    assert!(worker.sandbox().is_placeholder());
}

#[tokio::test]
async fn worker_failed_turn_preserves_provider_error_chain() {
    let worker = AgentWorker::builder()
        .llm(Arc::new(FailingProvider))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = SessionManager::new().load_or_create_thread(&incoming).await;

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming,
            thread_context: ThreadContext::from_conversation_thread(
                ThreadContextLocator::from(&locator),
                ConversationThread::with_id(locator.thread_id, "default", Utc::now()),
            ),
            thread: ConversationThread::with_id(locator.thread_id, "default", Utc::now()),
            history: Vec::new(),
            loaded_toolsets: Vec::new(),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 1).await;

    match &events[0] {
        AgentWorkerEvent::TurnFailed(turn) => {
            assert!(turn.error.contains("failed to call llm provider"));
            assert!(turn.error.contains("demo-model"));
            assert!(
                turn.error
                    .contains("provider said 429: rate limit exceeded")
            );
        }
        other => panic!("unexpected first event: {other:?}"),
    }
}

async fn collect_events(
    mut event_rx: mpsc::Receiver<AgentWorkerEvent>,
    expected_count: usize,
) -> Vec<AgentWorkerEvent> {
    timeout(Duration::from_millis(500), async move {
        let mut events = Vec::new();
        while events.len() < expected_count {
            let event = event_rx
                .recv()
                .await
                .expect("agent event channel should stay open");
            events.push(event);
        }
        events
    })
    .await
    .expect("events should be emitted")
}

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: Some("tester".to_string()),
        content: content.to_string(),
        external_thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

fn thread_with_history(thread_id: uuid::Uuid, history: &[ChatMessage]) -> ConversationThread {
    let now = Utc::now();
    let mut thread = ConversationThread::with_id(thread_id, "default", now);
    if !history.is_empty() {
        thread.store_turn(None, history.to_vec(), now, now);
    }
    thread
}
