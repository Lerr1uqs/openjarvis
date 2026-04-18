use chrono::Utc;
use openjarvis::{
    agent::{AgentWorker, SubagentRequest},
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
        external_message_id: Some("msg_subagent".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_subagent".to_string(),
        user_name: None,
        content: content.to_string(),
        external_thread_id: Some("chat_subagent".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_subagent".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}

#[tokio::test]
async fn subagent_runner_executes_child_thread_with_internal_only_dispatch_events() {
    // 测试场景: SubagentRunner 必须在独立队列里同步返回结果，并把 committed event 标记成 internal-only。
    let sessions = SessionManager::new();
    let worker = AgentWorker::builder()
        .llm(Arc::new(MockLLMProvider::new("subagent reply")))
        .build()
        .expect("worker should build");
    sessions.install_thread_runtime(worker.thread_runtime());

    let incoming = build_incoming("parent hello");
    let parent_locator = sessions
        .create_thread(&incoming, ThreadAgentKind::Main)
        .await
        .expect("parent thread should resolve");
    let child_locator =
        ThreadLocator::for_child(&parent_locator, "browser", SubagentSpawnMode::Persist);
    let child_locator = sessions
        .create_thread_at(
            &child_locator,
            incoming.received_at,
            ThreadAgentKind::Browser,
        )
        .await
        .expect("child thread should resolve");

    let result = worker
        .subagent_runner()
        .run(SubagentRequest {
            parent_locator: parent_locator.clone(),
            child_locator: child_locator.clone(),
            prompt: "child hello".to_string(),
            sessions: sessions.clone(),
        })
        .await
        .expect("subagent request should succeed");

    assert_eq!(result.output.reply, "subagent reply");
    assert!(result.output.succeeded);
    assert!(!result.dispatch_events.is_empty());
    assert!(
        result
            .dispatch_events
            .iter()
            .all(|event| !event.channel_delivery_enabled
                && event.metadata["dispatch_scope"] == "subagent_internal")
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
            .any(|message| message.content == "child hello")
    );
    assert!(
        child_thread
            .messages()
            .iter()
            .any(|message| message.content == "subagent reply")
    );

    let parent_thread = sessions
        .load_thread(&parent_locator)
        .await
        .expect("parent thread should load")
        .expect("parent thread should exist");
    assert!(
        parent_thread
            .messages()
            .iter()
            .all(|message| message.content != "child hello")
    );
}
