use crate::support::TestTopicQueue;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentWorker, AgentWorkerEvent},
    context::ChatMessageRole,
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    queue::{TopicQueue, TopicQueuePayload},
    session::{SessionManager, ThreadLocator},
    thread::ThreadAgentKind,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

struct MainCallsSubagentProvider {
    call_index: AtomicUsize,
}

impl MainCallsSubagentProvider {
    fn new() -> Self {
        Self {
            call_index: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LLMProvider for MainCallsSubagentProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        match self.call_index.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool.name == "spawn_subagent"),
                    "main agent must see spawn_subagent tool"
                );
                assert_eq!(
                    request.messages.last().map(|message| message.role.clone()),
                    Some(ChatMessageRole::User)
                );
                assert_eq!(
                    request
                        .messages
                        .last()
                        .map(|message| message.content.as_str()),
                    Some("帮我让 demo agent 回一个 mock 结果")
                );
                Ok(LLMResponse {
                    items: vec![
                        openjarvis::context::ChatMessage::new(
                            ChatMessageRole::Toolcall,
                            "",
                            Utc::now(),
                        )
                        .with_tool_calls(vec![LLMToolCall {
                            id: "call_spawn_subagent".to_string(),
                            name: "spawn_subagent".to_string(),
                            arguments: json!({
                                "subagent_key": "browser",
                                "content": "请返回 demo agent 的 mock 结果",
                                "spawn_mode": "persist",
                            }),
                            provider_item_id: None,
                        }]),
                    ],
                })
            }
            1 => {
                assert_eq!(
                    request.messages.last().map(|message| message.role.clone()),
                    Some(ChatMessageRole::User)
                );
                assert_eq!(
                    request
                        .messages
                        .last()
                        .map(|message| message.content.as_str()),
                    Some("请返回 demo agent 的 mock 结果")
                );
                Ok(LLMResponse {
                    items: vec![openjarvis::context::ChatMessage::new(
                        ChatMessageRole::Assistant,
                        "demo agent mock result",
                        Utc::now(),
                    )],
                })
            }
            2 => {
                assert!(
                    request.messages.iter().any(|message| {
                        message.role == ChatMessageRole::ToolResult
                            && message.content == "demo agent mock result"
                    }),
                    "main agent second round must receive child tool result"
                );
                Ok(LLMResponse {
                    items: vec![openjarvis::context::ChatMessage::new(
                        ChatMessageRole::Assistant,
                        "main agent received demo agent mock result",
                        Utc::now(),
                    )],
                })
            }
            other => Err(anyhow!("unexpected llm generate call index `{other}`")),
        }
    }
}

async fn enqueue_worker_request(
    queue: &TestTopicQueue,
    sessions: &SessionManager,
    incoming: &IncomingMessage,
) -> ThreadLocator {
    let locator = sessions
        .create_thread(incoming, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    queue
        .add(
            &locator.thread_key(),
            TopicQueuePayload::new(locator.clone(), incoming.clone()),
        )
        .await
        .expect("message should enter topic queue");
    locator
}

#[tokio::test]
async fn worker_emits_dispatch_then_request_completed() {
    // 测试场景: domain worker 从 queue claim 后，先发 committed dispatch，再在 queue complete 成功后发 request completed。
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::default());
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("mock-reply")))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn(queue.clone());
    let incoming = build_incoming("hello");
    let locator = enqueue_worker_request(&queue, &sessions, &incoming).await;

    handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("domain worker should start");

    let first = timeout(
        Duration::from_secs(5),
        handle
            .event_rx_mut()
            .expect("worker event receiver should be available")
            .recv(),
    )
    .await
    .expect("first event should arrive")
    .expect("first event should exist");
    let second = timeout(
        Duration::from_secs(5),
        handle
            .event_rx_mut()
            .expect("worker event receiver should be available")
            .recv(),
    )
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

    assert!(
        timeout(
            Duration::from_millis(300),
            handle
                .event_rx_mut()
                .expect("worker event receiver should be available")
                .recv(),
        )
        .await
        .is_err(),
        "worker should emit only dispatch and completion for one queue message"
    );
}

#[tokio::test]
async fn worker_reports_failed_request_completion_after_fallback_message() {
    // 测试场景: 硬失败时 worker 仍写入 thread-owned fallback reply，并在 queue complete 后把请求标记为 failed。
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::default());
    let worker = AgentWorker::builder()
        .llm(Arc::new(FailingProvider))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn(queue.clone());
    let incoming = build_incoming("hello");
    let locator = enqueue_worker_request(&queue, &sessions, &incoming).await;

    handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("domain worker should start");

    let mut saw_fallback_dispatch = false;
    let mut saw_failed_completion = false;
    for _ in 0..2 {
        let event = timeout(
            Duration::from_secs(5),
            handle
                .event_rx_mut()
                .expect("worker event receiver should be available")
                .recv(),
        )
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
        .load_thread(&locator)
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

#[tokio::test]
async fn worker_runs_full_main_to_subagent_roundtrip_with_mock_result() {
    // 测试场景: queue 驱动的主线程 worker 仍能完整跑通 spawn_subagent 往返，并在结束后写回 child/main thread。
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::default());
    let worker = AgentWorker::builder()
        .llm(Arc::new(MainCallsSubagentProvider::new()))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn(queue.clone());
    let incoming = build_incoming("帮我让 demo agent 回一个 mock 结果");
    let locator = enqueue_worker_request(&queue, &sessions, &incoming).await;

    handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("domain worker should start");

    let mut dispatch_contents = Vec::new();
    loop {
        let event = timeout(
            Duration::from_secs(5),
            handle
                .event_rx_mut()
                .expect("worker event receiver should be available")
                .recv(),
        )
        .await
        .expect("worker event should arrive")
        .expect("worker event should exist");
        match event {
            AgentWorkerEvent::DispatchItemCommitted(item) => {
                assert_ne!(
                    item.dispatch_event.metadata["dispatch_scope"],
                    "subagent_internal"
                );
                dispatch_contents.push(item.dispatch_event.content);
            }
            AgentWorkerEvent::RequestCompleted(completed) => {
                assert_eq!(completed.locator.thread_id, locator.thread_id);
                assert!(completed.succeeded);
                break;
            }
        }
    }

    assert!(
        dispatch_contents
            .iter()
            .any(|content| content.contains("[openjarvis][tool_call] spawn_subagent"))
    );
    assert!(
        dispatch_contents
            .iter()
            .any(|content| content.contains("demo agent mock result"))
    );
    assert!(
        dispatch_contents
            .iter()
            .any(|content| content == "main agent received demo agent mock result")
    );

    let main_thread = sessions
        .load_thread(&locator)
        .await
        .expect("main thread should load")
        .expect("main thread should exist");
    assert!(
        main_thread
            .messages()
            .iter()
            .any(|message| message.role == ChatMessageRole::ToolResult
                && message.content == "demo agent mock result")
    );
    assert!(
        main_thread
            .messages()
            .iter()
            .any(|message| message.role == ChatMessageRole::Assistant
                && message.content == "main agent received demo agent mock result")
    );

    let child_locator = ThreadLocator::for_child(
        &locator,
        "browser",
        openjarvis::thread::SubagentSpawnMode::Persist,
    );
    let child_thread = sessions
        .load_thread(&child_locator)
        .await
        .expect("child thread should load")
        .expect("child thread should exist");
    assert!(
        child_thread
            .messages()
            .iter()
            .any(|message| message.role == ChatMessageRole::Assistant
                && message.content == "demo agent mock result")
    );
}
