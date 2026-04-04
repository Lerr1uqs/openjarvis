use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{AgentRequest, AgentRuntime, AgentWorker, AgentWorkerBuilder, AgentWorkerEvent},
    config::{AppConfig, install_global_config},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, MockLLMProvider},
    model::{IncomingMessage, ReplyTarget},
    session::SessionManager,
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};
use uuid::Uuid;

#[tokio::test]
async fn worker_spawn_emits_outgoing_and_completed_commit() {
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("mock-reply")))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = SessionManager::new()
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: incoming.clone(),
            thread_context: Thread::new(ThreadContextLocator::from(&locator), incoming.received_at),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 3).await;

    match &events[0] {
        AgentWorkerEvent::ThreadContextSynced(update) => {
            assert_eq!(update.locator.thread_id, locator.thread_id);
            assert_eq!(
                update.thread_context.system_prefix_messages()[0].content,
                "system prompt"
            );
            assert!(
                update
                    .thread_context
                    .system_prefix_messages()
                    .iter()
                    .any(|message| message.content.contains("OpenJarvis tool-use mode"))
            );
        }
        other => panic!("unexpected first event: {other:?}"),
    }

    match &events[1] {
        AgentWorkerEvent::Dispatch(event) => {
            assert_eq!(event.content, "mock-reply");
            assert_eq!(event.external_thread_id, None);
            assert_eq!(event.session_external_thread_id, "default");
            assert_eq!(format!("{:?}", event.kind), "TextOutput");
            assert!(event.reply_to_source);
            assert_eq!(event.source_message_id.as_deref(), Some("msg_1"));
        }
        other => panic!("unexpected second event: {other:?}"),
    }

    match &events[2] {
        AgentWorkerEvent::CommitCompleted(commit) => {
            assert_eq!(commit.locator.thread_id, locator.thread_id);
            assert_eq!(commit.commit_messages.len(), 1);
            assert_eq!(commit.commit_messages[0].content, "mock-reply");
        }
        other => panic!("unexpected third event: {other:?}"),
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
    let locator = SessionManager::new()
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");
    let mut thread_context = thread_with_history(
        locator.thread_id,
        &[ChatMessage::new(
            ChatMessageRole::Assistant,
            "previous reply",
            Utc::now(),
        )],
    );
    thread_context.rebind_locator(ThreadContextLocator::from(&locator));
    let _ = thread_context.ensure_system_prompt_snapshot("system prompt", Utc::now());

    handle
        .request_tx
        .send(AgentRequest {
            // 测试场景: 非空 thread 在进入 worker 前应已带上持久化 system prompt snapshot，
            // loop 不再对这类线程做补初始化。
            thread_context,
            locator,
            incoming,
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
async fn worker_can_build_from_explicit_and_global_config_paths() {
    // 测试场景: runtime/worker 显式 from_config 路径必须继续可用，同时主启动链路可以切换到全局配置便捷入口。
    let config = AppConfig::from_yaml_str(
        r#"
agent:
  hook:
    notification: ["echo", "worker-hook"]
llm:
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
async fn worker_failed_commit_preserves_provider_error_chain() {
    let worker = AgentWorker::builder()
        .llm(Arc::new(FailingProvider))
        .system_prompt("system prompt")
        .build()
        .expect("worker should build");
    let handle = worker.spawn();
    let incoming = build_incoming("hello");
    let locator = SessionManager::new()
        .load_or_create_thread(&incoming)
        .await
        .expect("thread should resolve");

    handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming,
            thread_context: Thread::new(ThreadContextLocator::from(&locator), Utc::now()),
        })
        .await
        .expect("request should be accepted");

    let events = collect_events(handle.event_rx, 2).await;

    match &events[0] {
        AgentWorkerEvent::ThreadContextSynced(update) => {
            assert_eq!(update.locator.thread_id, locator.thread_id);
            assert_eq!(
                update.thread_context.system_prefix_messages()[0].content,
                "system prompt"
            );
        }
        other => panic!("unexpected first event: {other:?}"),
    }

    match &events[1] {
        AgentWorkerEvent::CommitFailed(commit) => {
            assert!(commit.error.contains("failed to call llm provider"));
            assert!(commit.error.contains("demo-model"));
            assert!(
                commit
                    .error
                    .contains("provider said 429: rate limit exceeded")
            );
        }
        other => panic!("unexpected second event: {other:?}"),
    }
}

#[tokio::test]
async fn worker_preserves_existing_thread_system_prompt_snapshot() {
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
    let locator = SessionManager::new()
        .load_or_create_thread(&first_incoming)
        .await
        .expect("thread should resolve");

    first_handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: first_incoming.clone(),
            thread_context: Thread::new(
                ThreadContextLocator::from(&locator),
                first_incoming.received_at,
            ),
        })
        .await
        .expect("first request should be accepted");

    let first_events = collect_events(first_handle.event_rx, 3).await;
    let preserved_thread_context = match &first_events[2] {
        AgentWorkerEvent::CommitCompleted(commit) => {
            assert_eq!(
                commit.thread_context.system_prefix_messages()[0].content,
                "system prompt A"
            );
            commit.thread_context.clone()
        }
        other => panic!("unexpected first worker completion event: {other:?}"),
    };
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
            thread_context: preserved_thread_context.clone(),
        })
        .await
        .expect("second request should be accepted");

    let second_events = collect_events(second_handle.event_rx, 2).await;
    match &second_events[1] {
        AgentWorkerEvent::CommitCompleted(commit) => {
            assert_eq!(
                commit.thread_context.system_prefix_messages()[0].content,
                "system prompt A"
            );
        }
        other => panic!("unexpected second worker completion event: {other:?}"),
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

fn thread_with_history(thread_id: uuid::Uuid, history: &[ChatMessage]) -> Thread {
    let now = Utc::now();
    let mut thread = Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
        now,
    );
    if !history.is_empty() {
        thread.store_turn(None, history.to_vec(), now, now);
    }
    thread
}
