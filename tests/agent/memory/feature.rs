use super::super::support::ThreadTestExt;
use super::MemoryWorkspaceFixture;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentRequest, AgentRuntime, AgentWorker, AgentWorkerEvent, HookRegistry, ToolRegistry,
    },
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest, LLMResponse, LLMToolCall},
    model::{IncomingMessage, ReplyTarget},
    session::{SessionKey, SessionManager, ThreadLocator},
    thread::Thread,
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
    let mut writer_handle = writer_worker.spawn();

    let remember_incoming = build_incoming("请记住 notion 上传工作流");
    let locator = build_locator(&remember_incoming);
    writer_handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: remember_incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("writer request should be accepted");

    let writer_events = collect_until_commit(
        writer_handle
            .event_rx_mut()
            .expect("writer event receiver should be available"),
    )
    .await;
    let remembered_thread = extract_completed_thread(&writer_events);
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
    let mut reader_handle = reader_worker.spawn();

    let same_thread_incoming = build_incoming("notion 细节是什么");
    reader_handle
        .request_tx
        .send(AgentRequest {
            locator: locator.clone(),
            incoming: same_thread_incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("same-thread follow-up should be accepted");
    let _ = collect_until_commit(
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
        cleared_thread.clear_to_initial_state(Utc::now());
        sessions
            .persist_locked_thread_context(
                &locator,
                &mut cleared_thread,
                reinit_incoming.received_at,
            )
            .await
            .expect("cleared thread should store");
    }
    reader_handle
        .request_tx
        .send(AgentRequest {
            locator,
            incoming: reinit_incoming.clone(),
            sessions: sessions.clone(),
        })
        .await
        .expect("reinit follow-up should be accepted");
    let reinit_events = collect_until_commit(
        reader_handle
            .event_rx_mut()
            .expect("reader event receiver should be available"),
    )
    .await;
    let reinit_thread = extract_completed_thread(&reinit_events);
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

fn build_locator(incoming: &IncomingMessage) -> ThreadLocator {
    let thread_key = "default";
    ThreadLocator::new(
        Uuid::new_v4(),
        incoming,
        thread_key,
        SessionKey::from_incoming(incoming).derive_thread_id(thread_key),
    )
}

async fn collect_until_commit(
    event_rx: &mut mpsc::Receiver<AgentWorkerEvent>,
) -> Vec<AgentWorkerEvent> {
    timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        loop {
            let event = event_rx
                .recv()
                .await
                .expect("agent worker event channel should stay open");
            let terminal = matches!(event, AgentWorkerEvent::TurnFinalized(_));
            events.push(event);
            if terminal {
                break events;
            }
        }
    })
    .await
    .expect("agent worker events should arrive in time")
}

fn extract_completed_thread(events: &[AgentWorkerEvent]) -> Thread {
    events
        .iter()
        .find_map(|event| match event {
            AgentWorkerEvent::TurnFinalized(turn) => Some(turn.turn.snapshot.clone()),
            _ => None,
        })
        .expect("turn finalized event should exist")
}
