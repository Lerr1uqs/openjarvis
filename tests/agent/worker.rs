use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentRequest, AgentWorker, AgentWorkerEvent},
    context::ChatMessageRole,
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::{SessionManager, ThreadLocator},
    thread::ThreadAgentKind,
};
use serde_json::json;
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
                    request.tools.iter().any(|tool| tool.name == "send_subagent"),
                    "main agent must see send_subagent tool"
                );
                assert_eq!(
                    request.messages.last().map(|message| message.role.clone()),
                    Some(ChatMessageRole::User)
                );
                assert_eq!(
                    request.messages.last().map(|message| message.content.as_str()),
                    Some("帮我让 demo agent 回一个 mock 结果")
                );
                Ok(LLMResponse {
                    items: vec![
                        openjarvis::context::ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now())
                            .with_tool_calls(vec![LLMToolCall {
                                id: "call_send_subagent".to_string(),
                                name: "send_subagent".to_string(),
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
                    request.messages.last().map(|message| message.content.as_str()),
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

#[tokio::test]
async fn worker_emits_dispatch_then_request_completed() {
    // 测试场景: worker 事件流只保留 committed dispatch 与 request completion，不再发 finalized turn。
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(std::sync::Arc::new(MockLLMProvider::new("mock-reply")))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
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
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
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
    // 测试场景: mock main agent 通过 send_subagent 调起 mock demo agent，并把 child result 带回主线程继续生成最终回复。
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(std::sync::Arc::new(MainCallsSubagentProvider::new()))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn();
    let incoming = build_incoming("帮我让 demo agent 回一个 mock 结果");
    let locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("main thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("request should be accepted");

    let mut dispatch_contents = Vec::new();
    loop {
        let event = timeout(Duration::from_secs(5), handle.event_rx.recv())
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
            .any(|content| content.contains("[openjarvis][tool_call] send_subagent"))
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

    let child_locator =
        ThreadLocator::for_child(&locator, "browser", openjarvis::thread::SubagentSpawnMode::Persist);
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
