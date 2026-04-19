use chrono::Utc;
use openjarvis::{
    agent::{AgentWorker, ToolCallRequest},
    llm::MockLLMProvider,
    model::{IncomingMessage, ReplyTarget},
    session::{SessionManager, ThreadLocator},
    thread::{SubagentSpawnMode, ThreadAgentKind},
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn build_incoming(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_subagent_tool".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_subagent_tool".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_subagent_tool".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_subagent_tool".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

async fn build_parent_thread() -> (
    AgentWorker,
    SessionManager,
    ThreadLocator,
    openjarvis::thread::Thread,
) {
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("tool child reply")))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());
    worker
        .runtime()
        .tools()
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");

    let incoming = build_incoming("parent hello");
    let parent_locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");
    let mut parent_thread = sessions
        .load_thread(&parent_locator)
        .await
        .expect("parent thread should load")
        .expect("parent thread should exist");
    parent_thread.bind_request_runtime(sessions.clone());
    (worker, sessions, parent_locator, parent_thread)
}

#[tokio::test]
async fn subagent_tools_are_visible_only_on_parent_threads() {
    // 测试场景: 四个 subagent 工具只允许主线程看到，child thread 自己不能再次看到这组管理接口。
    let (worker, sessions, parent_locator, parent_thread) = build_parent_thread().await;
    let parent_tools = worker
        .runtime()
        .tools()
        .list_for_context(&parent_thread)
        .await
        .expect("parent tools should list");
    assert!(
        parent_tools
            .iter()
            .any(|tool| tool.name == "spawn_subagent")
    );
    assert!(parent_tools.iter().any(|tool| tool.name == "send_subagent"));
    assert!(
        parent_tools
            .iter()
            .any(|tool| tool.name == "close_subagent")
    );
    assert!(parent_tools.iter().any(|tool| tool.name == "list_subagent"));

    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_locator = sessions
        .create_thread_at(&child_locator, Utc::now(), ThreadAgentKind::Browser)
        .await
        .expect("child thread should resolve");
    let child_thread = sessions
        .load_thread(&child_locator)
        .await
        .expect("child thread should load")
        .expect("child thread should exist");
    let child_tools = worker
        .runtime()
        .tools()
        .list_for_context(&child_thread)
        .await
        .expect("child tools should list");
    assert!(!child_tools.iter().any(|tool| tool.name == "spawn_subagent"));
    assert!(!child_tools.iter().any(|tool| tool.name == "send_subagent"));
}

#[tokio::test]
async fn subagent_tools_manage_persist_lifecycle_and_list_view() {
    // 测试场景: persist subagent 应由 spawn 完成首轮执行，send 只做后续交互，close 后列表仍保留 identity 但不可继续发送。
    let (worker, _sessions, parent_locator, mut parent_thread) = build_parent_thread().await;
    let registry = worker.runtime().tools();

    let spawn_result = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "spawn_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                    "content": "child hello",
                    "spawn_mode": "persist",
                }),
            },
        )
        .await
        .expect("spawn_subagent should succeed");
    assert!(!spawn_result.is_error);
    assert_eq!(spawn_result.content, "tool child reply");

    let send_result = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "send_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                    "content": "child follow-up",
                }),
            },
        )
        .await
        .expect("send_subagent should succeed");
    assert_eq!(send_result.content, "tool child reply");
    assert!(!send_result.is_error);

    let list_result = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "list_subagent".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("list_subagent should succeed");
    let subagents = list_result.metadata["subagents"]
        .as_array()
        .expect("subagent list should be an array");
    assert_eq!(subagents.len(), 1);
    assert_eq!(subagents[0]["subagent_key"], "browser");
    assert_eq!(subagents[0]["available"], true);

    let close_result = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "close_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                }),
            },
        )
        .await
        .expect("close_subagent should succeed");
    assert!(!close_result.is_error);

    let list_after_close = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "list_subagent".to_string(),
                arguments: json!({}),
            },
        )
        .await
        .expect("list_subagent after close should succeed");
    let subagents_after_close = list_after_close.metadata["subagents"]
        .as_array()
        .expect("subagent list should remain an array");
    assert_eq!(subagents_after_close.len(), 1);
    assert_eq!(subagents_after_close[0]["available"], false);

    let send_after_close = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "send_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                    "content": "should fail after close",
                }),
            },
        )
        .await
        .expect_err("send_subagent after close should fail");
    assert!(
        send_after_close
            .to_string()
            .contains("call spawn_subagent to reinitialize it first")
    );

    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    assert_eq!(
        child_locator.thread_id.to_string(),
        subagents_after_close[0]["thread_id"]
            .as_str()
            .expect("child thread id should exist")
    );
}

#[tokio::test]
async fn send_subagent_requires_existing_persist_child_thread() {
    // 测试场景: send_subagent 不再承担首轮创建职责；缺失 persist child 时必须显式失败。
    let (worker, _sessions, _parent_locator, mut parent_thread) = build_parent_thread().await;
    let registry = worker.runtime().tools();

    let error = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "send_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                    "content": "first task should fail",
                }),
            },
        )
        .await
        .expect_err("send_subagent without spawn should fail");
    assert!(error.to_string().contains("call spawn_subagent first"));
}

#[tokio::test]
async fn spawn_subagent_yolo_removes_child_thread_after_completion() {
    // 测试场景: yolo subagent 通过一次 spawn 完成执行后，必须 best-effort 删除 child thread 记录。
    let (worker, sessions, parent_locator, mut parent_thread) = build_parent_thread().await;
    let registry = worker.runtime().tools();

    let spawn_result = registry
        .call_for_context(
            &mut parent_thread,
            ToolCallRequest {
                name: "spawn_subagent".to_string(),
                arguments: json!({
                    "subagent_key": "browser",
                    "content": "child hello",
                    "spawn_mode": "yolo",
                }),
            },
        )
        .await
        .expect("yolo spawn_subagent should succeed");
    assert!(!spawn_result.is_error);
    assert_eq!(spawn_result.metadata["cleanup_removed"], true);

    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Yolo);
    assert!(
        sessions
            .load_thread(&child_locator)
            .await
            .expect("yolo child load should resolve")
            .is_none()
    );
}
