use super::{memory::MemoryWorkspaceFixture, support::ThreadTestExt};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentRequest, AgentRuntime, AgentWorker, AgentWorkerBuilder, AgentWorkerEvent,
        HookRegistry, MemoryType, MemoryWriteRequest, ToolRegistry,
    },
    config::{AppConfig, install_global_config},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::SessionManager,
    thread::{Thread, ThreadContextLocator, ThreadFinalizedTurnStatus},
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};
use uuid::Uuid;

#[tokio::test]
async fn worker_spawn_emits_thread_sync_then_finalized_turn() {
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("mock-reply")))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
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

    let events = collect_events(handle.event_rx, 4).await;

    match &events[0] {
        AgentWorkerEvent::MessageCommitted(message) => {
            assert_eq!(message.message.content, "hello");
            assert!(message.dispatch_events.is_empty());
        }
        other => panic!("unexpected first event: {other:?}"),
    }

    match &events[1] {
        AgentWorkerEvent::MessageCommitted(message) => {
            assert_eq!(message.message.content, "mock-reply");
            assert_eq!(message.dispatch_events.len(), 1);
            assert_eq!(message.dispatch_events[0].content, "mock-reply");
        }
        other => panic!("unexpected second event: {other:?}"),
    }

    match &events[2] {
        AgentWorkerEvent::TurnFinalized(turn) => {
            assert_eq!(turn.locator.thread_id, locator.thread_id);
            assert_eq!(
                turn.turn
                    .snapshot
                    .non_system_messages()
                    .iter()
                    .map(|message| message.content.clone())
                    .collect::<Vec<_>>(),
                vec!["hello".to_string(), "mock-reply".to_string()]
            );
        }
        other => panic!("unexpected third event: {other:?}"),
    }

    match &events[3] {
        AgentWorkerEvent::RequestCompleted(completed) => {
            assert_eq!(completed.locator.thread_id, locator.thread_id);
        }
        other => panic!("unexpected fourth event: {other:?}"),
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
async fn worker_builds_request_from_thread_messages_plus_current_user_turn() {
    let sessions = SessionManager::new();
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
    let locator = sessions
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let now = Utc::now();
    let mut thread_context = Thread::new(ThreadContextLocator::from(&locator), now);
    thread_context.seed_persisted_messages(vec![ChatMessage::new(
        ChatMessageRole::System,
        "system prompt",
        now,
    )]);
    thread_context.commit_test_turn(
        None,
        vec![ChatMessage::new(
            ChatMessageRole::Assistant,
            "previous reply",
            now,
        )],
        now,
        now,
    );
    sessions
        .store_thread_context(&locator, thread_context, incoming.received_at)
        .await
        .expect("seed thread context should store");

    handle
        .request_tx
        .send(AgentRequest {
            locator,
            incoming,
            sessions: sessions.clone(),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 4).await;
    match &events[2] {
        AgentWorkerEvent::TurnFinalized(_) => {}
        other => panic!("unexpected third event: {other:?}"),
    }
    match &events[3] {
        AgentWorkerEvent::RequestCompleted(_) => {}
        other => panic!("unexpected fourth event: {other:?}"),
    }
    let captured_requests = requests.lock().await;
    let messages = &captured_requests[0].messages;

    assert!(
        messages
            .iter()
            .any(|message| message.content == "system prompt")
    );
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
  protocol: "mock"
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
async fn worker_can_build_from_explicit_and_global_config_paths() {
    // 测试场景: runtime/worker 显式 from_config 路径必须继续可用，同时主启动链路可以切换到全局配置便捷入口。
    let config = AppConfig::from_yaml_str(
        r#"
agent:
  hook:
    notification: ["echo", "worker-hook"]
llm:
  protocol: "mock"
  provider: "mock"
"#,
    )
    .expect("config should parse");

    let explicit_runtime = AgentRuntime::from_config(config.agent_config())
        .await
        .expect("explicit runtime should build");
    assert_eq!(explicit_runtime.hooks().len().await, 1);

    let explicit_worker = AgentWorker::from_config(&config)
        .await
        .expect("explicit worker should build");
    assert_eq!(explicit_worker.runtime().hooks().len().await, 1);

    install_global_config(config).expect("global config should install");
    let global_runtime = AgentRuntime::from_global_config()
        .await
        .expect("global runtime should build");
    assert_eq!(global_runtime.hooks().len().await, 1);
    let global_worker = AgentWorker::from_global_config()
        .await
        .expect("global worker should build");
    assert_eq!(global_worker.runtime().hooks().len().await, 1);
    assert!(global_worker.sandbox().is_placeholder());
}

#[tokio::test]
async fn worker_failed_turn_is_finalized_inside_thread_boundary() {
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(Arc::new(FailingProvider))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = sessions
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming,
            sessions: sessions.clone(),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 4).await;

    match &events[2] {
        AgentWorkerEvent::TurnFinalized(turn) => {
            assert!(matches!(
                turn.turn.status,
                ThreadFinalizedTurnStatus::Failed { .. }
            ));
            assert_eq!(turn.turn.snapshot.non_system_messages().len(), 2);
            assert_eq!(turn.turn.snapshot.non_system_messages()[0].content, "hello");
            assert!(
                turn.turn.snapshot.non_system_messages()[1]
                    .content
                    .contains("provider said 429: rate limit exceeded")
            );
        }
        other => panic!("unexpected third event: {other:?}"),
    }

    match &events[3] {
        AgentWorkerEvent::RequestCompleted(completed) => {
            assert_eq!(completed.locator.thread_id, locator.thread_id);
        }
        other => panic!("unexpected fourth event: {other:?}"),
    }
}

#[tokio::test]
async fn worker_preserves_existing_thread_system_prompt_snapshot() {
    let sessions = SessionManager::new();
    // 测试场景: 旧线程首次补齐 system prompt snapshot 后，后续轮次即使 worker 配置变化，也必须继续使用原快照。
    let first_requests = Arc::new(Mutex::new(Vec::new()));
    let first_worker = AgentWorker::builder()
        .llm(Arc::new(RecordingProvider {
            requests: Arc::clone(&first_requests),
        }))
        .system_prompt("system prompt A")
        .build()
        .expect("first worker should build");
    let first_handle = first_worker.spawn();
    let first_incoming = build_incoming("hello");
    let locator = sessions
        .load_or_create_thread(&first_incoming)
        .await
        .expect("thread should resolve");

    first_handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: first_incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("first request should be accepted");

    let first_events = collect_events(first_handle.event_rx, 4).await;
    match &first_events[2] {
        AgentWorkerEvent::TurnFinalized(turn) => {
            assert_eq!(
                turn.turn.snapshot.system_messages()[0].content,
                "system prompt A"
            );
        }
        other => panic!("unexpected first worker completion event: {other:?}"),
    };
    match &first_events[3] {
        AgentWorkerEvent::RequestCompleted(_) => {}
        other => panic!("unexpected fourth event: {other:?}"),
    }
    let first_captured_requests = first_requests.lock().await;
    assert_eq!(
        first_captured_requests[0].messages[0].content,
        "system prompt A"
    );
    drop(first_captured_requests);

    let second_requests = Arc::new(Mutex::new(Vec::new()));
    let second_worker = AgentWorker::builder()
        .llm(Arc::new(RecordingProvider {
            requests: Arc::clone(&second_requests),
        }))
        .system_prompt("system prompt B")
        .build()
        .expect("second worker should build");
    let second_handle = second_worker.spawn();
    let second_incoming = build_incoming("follow up");

    second_handle
        .request_tx
        .send(AgentRequest {
            locator,
            incoming: second_incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("second request should be accepted");

    let second_events = collect_events(second_handle.event_rx, 4).await;
    match &second_events[2] {
        AgentWorkerEvent::TurnFinalized(turn) => {
            assert_eq!(
                turn.turn.snapshot.system_messages()[0].content,
                "system prompt A"
            );
        }
        other => panic!("unexpected third worker completion event: {other:?}"),
    }
    match &second_events[3] {
        AgentWorkerEvent::RequestCompleted(_) => {}
        other => panic!("unexpected fourth event: {other:?}"),
    }

    let second_captured_requests = second_requests.lock().await;
    assert_eq!(
        second_captured_requests[0].messages[0].content,
        "system prompt A"
    );
    assert_eq!(
        second_captured_requests[0]
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("follow up")
    );
}

#[tokio::test]
async fn worker_does_not_directly_inject_memory_bodies_into_request_messages() {
    // 测试场景: active memory 只以初始化 catalog 形式暴露；worker 不能因为命中 keyword 就直接把正文拼进请求。
    let sessions = SessionManager::new();
    let fixture = MemoryWorkspaceFixture::new("openjarvis-worker-request-time-memory");
    let registry = Arc::new(ToolRegistry::with_workspace_root_and_skill_roots(
        fixture.root(),
        Vec::new(),
    ));
    registry
        .memory_repository()
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "workflow/notion.md".to_string(),
            title: "Notion 上传工作流".to_string(),
            content: "上传到 notion 时走用户自定义模板".to_string(),
            keywords: Some(vec!["notion".to_string(), "上传".to_string()]),
        })
        .expect("active memory fixture should write");
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), Arc::clone(&registry));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let worker = AgentWorker::with_runtime(
        Arc::new(RecordingProvider {
            requests: Arc::clone(&requests),
        }),
        "system prompt",
        runtime,
    );
    let handle = worker.spawn();
    let incoming = build_incoming("notion 上传细节是什么");
    let locator = sessions
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator,
            incoming,
            sessions,
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 4).await;
    match &events[2] {
        AgentWorkerEvent::TurnFinalized(turn) => {
            assert!(turn.turn.snapshot.system_messages().iter().any(|message| {
                message
                    .content
                    .contains("notion, 上传 -> workflow/notion.md")
            }));
        }
        other => panic!("unexpected third event: {other:?}"),
    }

    let captured_requests = requests.lock().await;
    assert!(captured_requests[0].messages.iter().any(|message| {
        message
            .content
            .contains("notion, 上传 -> workflow/notion.md")
    }));
    assert!(!captured_requests[0].messages.iter().any(|message| {
        message.content.contains("上传到 notion 时走用户自定义模板")
    }));
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
