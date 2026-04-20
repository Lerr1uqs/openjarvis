use super::{
    HybridMockServer, MemoryWorkspaceFixture, build_config_from_yaml, hybrid_config_yaml,
    install_fixture_as_cwd, seed_hybrid_memory_corpus,
};
use crate::support::{TestTopicQueue, ThreadTestExt};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentRuntime, AgentWorker, AgentWorkerEvent, HookRegistry, ToolRegistry,
    },
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall},
    model::{IncomingMessage, ReplyTarget},
    queue::{TopicQueue, TopicQueuePayload},
    session::{SessionManager, ThreadLocator},
    thread::ThreadAgentKind,
};
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    sync::{Mutex, mpsc},
    time::{Duration, timeout},
};
use uuid::Uuid;

struct ScriptedLLMProvider {
    responses: Mutex<VecDeque<LLMResponse>>,
}

impl ScriptedLLMProvider {
    fn new(responses: Vec<LLMResponse>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }
}

#[async_trait]
impl LLMProvider for ScriptedLLMProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("no scripted memory response left"))
    }
}

struct RecordingProvider {
    requests: Arc<Mutex<Vec<LLMRequest>>>,
}

#[async_trait]
impl LLMProvider for RecordingProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        self.requests.lock().await.push(request);
        Ok(scripted_llm_response(
            Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                "ok",
                Utc::now(),
            )),
            Vec::new(),
        ))
    }
}

fn scripted_tool_call(id: &str, name: &str, arguments: serde_json::Value) -> LLMToolCall {
    LLMToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
        provider_item_id: None,
    }
}

fn scripted_llm_response(
    message: Option<ChatMessage>,
    tool_calls: Vec<LLMToolCall>,
) -> LLMResponse {
    let mut items = Vec::new();
    if let Some(message) = message {
        items.push(message);
    }
    items.extend(tool_calls.into_iter().map(|tool_call| {
        ChatMessage::new(ChatMessageRole::Toolcall, "", Utc::now()).with_tool_calls(vec![tool_call])
    }));
    LLMResponse { items }
}

#[tokio::test]
async fn active_memory_write_persists_to_filesystem_and_only_reappears_after_reinit() {
    // 测试场景: 用户触发 active memory 写入后，记忆要落盘；当前线程不热更新 catalog，清空重初始化后才把 grouped keywords->path 注入 system prompt。
    let sessions = SessionManager::new();
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-feature-worker");
    let writer_queue = Arc::new(TestTopicQueue::default());
    let reader_queue = Arc::new(TestTopicQueue::default());
    let registry = Arc::new(ToolRegistry::with_workspace_root_and_skill_roots(
        fixture.root(),
        Vec::new(),
    ));
    let runtime = AgentRuntime::with_parts(Arc::new(HookRegistry::new()), Arc::clone(&registry));

    let writer_worker = AgentWorker::with_runtime(
        Arc::new(ScriptedLLMProvider::new(vec![
            scripted_llm_response(
                Some(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "我先加载记忆工具",
                    Utc::now(),
                )),
                vec![scripted_tool_call(
                    "call_load_memory",
                    "load_toolset",
                    json!({ "name": "memory" }),
                )],
            ),
            scripted_llm_response(
                Some(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "正在写入记忆",
                    Utc::now(),
                )),
                vec![scripted_tool_call(
                    "call_memory_write",
                    "memory_write",
                    json!({
                        "path": "workflow/notion.md",
                        "title": "Notion 上传工作流",
                        "content": "上传到 notion 时走用户自定义模板",
                        "type": "active",
                        "keywords": ["notion", "上传"],
                    }),
                )],
            ),
            scripted_llm_response(
                Some(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    "记住了",
                    Utc::now(),
                )),
                Vec::new(),
            ),
        ])),
        runtime.clone(),
    );
    let thread_runtime = writer_worker.thread_runtime();
    sessions.install_thread_runtime(Arc::clone(&thread_runtime));
    let mut writer_handle = writer_worker.spawn(writer_queue.clone());

    let remember_incoming = build_incoming("请记住 notion 上传工作流");
    let locator = enqueue_new_request(&writer_queue, &sessions, &remember_incoming)
        .await
        .expect("writer request should be accepted");
    writer_handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("writer domain worker should start");

    collect_until_completion(
        writer_handle
            .event_rx_mut()
            .expect("writer event receiver should be available"),
    )
    .await;
    let remembered_thread = sessions
        .load_thread(&locator)
        .await
        .expect("remembered thread should load")
        .expect("remembered thread should exist");
    let memory_file = fixture.memory_root().join("active/workflow/notion.md");
    let written = std::fs::read_to_string(&memory_file).expect("memory file should exist");
    assert!(written.contains("Notion 上传工作流"));
    assert!(written.contains("上传到 notion 时走用户自定义模板"));
    assert!(!remembered_thread.system_messages().iter().any(|message| {
        message
            .content
            .contains("notion, 上传 -> workflow/notion.md")
    }));

    let requests = Arc::new(Mutex::new(Vec::new()));
    let reader_worker = AgentWorker::with_runtime(
        Arc::new(RecordingProvider {
            requests: Arc::clone(&requests),
        }),
        runtime,
    );
    let mut reader_handle = reader_worker.spawn(reader_queue.clone());

    let same_thread_incoming = build_incoming("notion 细节是什么");
    enqueue_existing_request(&reader_queue, &locator, &same_thread_incoming)
        .await
        .expect("same-thread follow-up should be accepted");
    reader_handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("reader domain worker should start");
    collect_until_completion(
        reader_handle
            .event_rx_mut()
            .expect("reader event receiver should be available"),
    )
    .await;

    let captured_requests = requests.lock().await;
    let same_thread_messages = &captured_requests[0].messages;
    assert!(!same_thread_messages.iter().any(|message| {
        message
            .content
            .contains("notion, 上传 -> workflow/notion.md")
    }));
    assert!(
        !same_thread_messages
            .iter()
            .any(|message| message.content.contains("上传到 notion 时走用户自定义模板"))
    );
    drop(captured_requests);

    let reinit_incoming = build_incoming("notion 细节是什么");
    {
        let mut cleared_thread = sessions
            .lock_thread(&locator, reinit_incoming.received_at)
            .await
            .expect("cleared thread lock result should resolve")
            .expect("cleared thread should lock");
        thread_runtime
            .reinitialize_thread(&mut cleared_thread, Utc::now())
            .await
            .expect("cleared thread should store");
    }
    enqueue_existing_request(&reader_queue, &locator, &reinit_incoming)
        .await
        .expect("reinit follow-up should be accepted");
    reader_handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("reader domain worker should stay available");
    collect_until_completion(
        reader_handle
            .event_rx_mut()
            .expect("reader event receiver should be available"),
    )
    .await;
    let reinit_thread = sessions
        .load_thread(&locator)
        .await
        .expect("reinitialized thread should load")
        .expect("reinitialized thread should exist");
    let captured_requests = requests.lock().await;
    let reinit_messages = &captured_requests[1].messages;

    assert!(reinit_thread.system_messages().iter().any(|message| {
        message
            .content
            .contains("notion, 上传 -> workflow/notion.md")
    }));
    assert!(reinit_messages.iter().any(|message| {
        message
            .content
            .contains("notion, 上传 -> workflow/notion.md")
    }));
    assert!(
        !reinit_messages
            .iter()
            .any(|message| message.content.contains("上传到 notion 时走用户自定义模板"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_search_presence_does_not_auto_inject_passive_memory_into_normal_requests() {
    // 测试场景: 即便工作区开启 hybrid retrieval，只要模型本轮没有显式调用 memory_search /
    // memory_get，普通请求上下文里仍然不能自动注入 passive memory 摘要或正文。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-feature-hybrid-no-auto-inject");
    seed_hybrid_memory_corpus(fixture.root());
    let mock_server = HybridMockServer::start(&fixture).await;
    let cwd_guard = install_fixture_as_cwd(fixture.root());
    let config = build_config_from_yaml(&hybrid_config_yaml(
        &mock_server.base_url(),
        mock_server
            .api_key_path()
            .to_str()
            .expect("api key path should be utf-8"),
        "",
    ));
    let runtime = AgentRuntime::from_config_with_skill_roots(config.agent_config(), Vec::new())
        .await
        .expect("agent runtime should build from hybrid config");
    let sessions = SessionManager::new();
    let queue = Arc::new(TestTopicQueue::default());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let worker = AgentWorker::with_runtime(
        Arc::new(RecordingProvider {
            requests: Arc::clone(&requests),
        }),
        runtime,
    );
    sessions.install_thread_runtime(worker.thread_runtime());
    let mut handle = worker.spawn(queue.clone());
    let incoming = build_incoming("以后回答时记住我的表达方式");
    let locator = enqueue_new_request(&queue, &sessions, &incoming)
        .await
        .expect("hybrid thread should resolve");

    handle
        .ensure_worker(locator.thread_key(), sessions.clone())
        .await
        .expect("hybrid domain worker should start");
    collect_until_completion(
        handle
            .event_rx_mut()
            .expect("worker event receiver should be available"),
    )
    .await;

    let captured_requests = requests.lock().await;
    let request_messages = &captured_requests[0].messages;
    assert!(!request_messages.iter().any(|message| {
        message
            .content
            .contains("默认使用中文，回答保持简洁，先给结论再展开细节")
    }));
    assert!(!request_messages.iter().any(|message| {
        message
            .content
            .contains("preferences/semantic-style-fresh.md")
    }));
    assert!(!request_messages.iter().any(|message| {
        message.content.contains("memory_search") && message.content.contains("semantic-style")
    }));
    drop(cwd_guard);
}

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(Uuid::new_v4().to_string()),
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

async fn enqueue_new_request(
    queue: &TestTopicQueue,
    sessions: &SessionManager,
    incoming: &IncomingMessage,
) -> Result<ThreadLocator> {
    let locator = sessions
        .create_thread(incoming, ThreadAgentKind::Main)
        .await
        .expect("thread should resolve");
    enqueue_existing_request(queue, &locator, incoming).await?;
    Ok(locator)
}

async fn enqueue_existing_request(
    queue: &TestTopicQueue,
    locator: &ThreadLocator,
    incoming: &IncomingMessage,
) -> Result<()> {
    queue
        .add(
            &locator.thread_key(),
            TopicQueuePayload::new(locator.clone(), incoming.clone()),
        )
        .await
        .map(|_| ())
}

async fn collect_until_completion(
    event_rx: &mut mpsc::Receiver<AgentWorkerEvent>,
) -> Vec<AgentWorkerEvent> {
    timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        loop {
            let event = event_rx
                .recv()
                .await
                .expect("agent worker event channel should stay open");
            let terminal = matches!(event, AgentWorkerEvent::RequestCompleted(_));
            events.push(event);
            if terminal {
                break events;
            }
        }
    })
    .await
    .expect("agent worker events should arrive in time")
}
