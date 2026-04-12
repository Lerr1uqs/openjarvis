use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentRequest, AgentWorker, AgentWorkerEvent},
    context::ChatMessageRole,
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::SessionManager,
};
use serde_json::json;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_worker".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_worker".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_worker".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_worker".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

struct FailingProvider;

#[async_trait]
impl LLMProvider for FailingProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        Err(anyhow!("upstream llm transport failed"))
    }
}

#[tokio::test]
async fn worker_emits_dispatch_then_request_completed() {
    // 测试场景: worker 事件流只保留 committed dispatch 与 request completion，不再发 finalized turn。
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(std::sync::Arc::new(MockLLMProvider::new("mock-reply")))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = sessions
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("request should be accepted");

    let first = timeout(Duration::from_secs(5), handle.event_rx.recv())
        .await
        .expect("first event should arrive")
        .expect("first event should exist");
    let second = timeout(Duration::from_secs(5), handle.event_rx.recv())
        .await
        .expect("second event should arrive")
        .expect("second event should exist");

    match first {
        AgentWorkerEvent::DispatchItemCommitted(item) => {
            assert_eq!(item.locator.thread_id, locator.thread_id);
            assert_eq!(item.dispatch_event.content, "mock-reply");
        }
        other => panic!("unexpected first event: {other:?}"),
    }

    match second {
        AgentWorkerEvent::RequestCompleted(completed) => {
            assert_eq!(completed.locator.thread_id, locator.thread_id);
            assert_eq!(completed.external_message_id.as_deref(), Some("msg_worker"));
            assert!(completed.succeeded);
        }
        other => panic!("unexpected second event: {other:?}"),
    }
}

#[tokio::test]
async fn worker_reports_failed_request_completion_after_fallback_message() {
    // 测试场景: 硬失败时 worker 仍通过 thread-owned message 提交 fallback reply，并把请求标记为 failed。
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(std::sync::Arc::new(FailingProvider))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = sessions
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("request should be accepted");

    let mut saw_fallback_dispatch = false;
    let mut saw_failed_completion = false;
    for _ in 0..2 {
        let event = timeout(Duration::from_secs(5), handle.event_rx.recv())
            .await
            .expect("worker event should arrive")
            .expect("worker event should exist");
        match event {
            AgentWorkerEvent::DispatchItemCommitted(item) => {
                saw_fallback_dispatch = true;
                assert!(
                    item.dispatch_event
                        .content
                        .contains("[openjarvis][agent_error]")
                );
            }
            AgentWorkerEvent::RequestCompleted(completed) => {
                saw_failed_completion = true;
                assert_eq!(completed.locator.thread_id, locator.thread_id);
                assert!(!completed.succeeded);
            }
        }
    }

    let thread = sessions
        .load_thread_context(&locator)
        .await
        .expect("thread should load")
        .expect("thread should exist");
    assert!(saw_fallback_dispatch);
    assert!(saw_failed_completion);
    assert!(
        thread
            .messages()
            .iter()
            .any(|message| message.role == ChatMessageRole::Assistant
                && message.content.contains("[openjarvis][agent_error]"))
    );
}
