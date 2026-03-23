use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentRequest, AgentWorker, AgentWorkerEvent},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::SessionManager,
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
    let worker = AgentWorker::new(
        Arc::new(MockLLMProvider::new("mock-reply")),
        "system prompt",
    );
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = SessionManager::new().load_or_create_thread(&incoming).await;

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            history: Vec::new(),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 2).await;

    match &events[0] {
        AgentWorkerEvent::Dispatch(event) => {
            assert_eq!(event.content, "mock-reply");
            assert_eq!(event.thread_id, None);
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

#[tokio::test]
async fn worker_builds_context_from_history_and_current_user_message() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let worker = AgentWorker::new(
        Arc::new(RecordingProvider {
            requests: Arc::clone(&requests),
        }),
        "system prompt",
    );
    let handle = worker.spawn();
    let incoming = build_incoming("what happened");
    let locator = SessionManager::new().load_or_create_thread(&incoming).await;

    handle
        .request_tx
        .send(AgentRequest {
            locator,
            incoming,
            history: vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                "previous reply",
                Utc::now(),
            )],
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
        thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}
