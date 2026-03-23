use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentDispatchEvent, AgentLoopEventKind, AgentRequest, AgentWorker, AgentWorkerEvent,
        AgentWorkerHandle, CompletedAgentTurn,
    },
    channels::{Channel, ChannelRegistration},
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
    llm::MockLLMProvider,
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    router::ChannelRouter,
    session::{SessionKey, SessionManager, SessionStrategy},
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

struct RecordingChannel {
    name: &'static str,
    sent: Arc<Mutex<Vec<OutgoingMessage>>>,
    incoming_tx: Arc<Mutex<Option<mpsc::Sender<IncomingMessage>>>>,
}

#[async_trait]
impl Channel for RecordingChannel {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn start(self: Arc<Self>, mut registration: ChannelRegistration) -> Result<()> {
        *self.incoming_tx.lock().await = Some(registration.incoming_tx);
        let sent = Arc::clone(&self.sent);
        tokio::spawn(async move {
            while let Some(message) = registration.outgoing_rx.recv().await {
                sent.lock().await.push(message);
            }
        });
        Ok(())
    }
}

#[tokio::test]
async fn router_ignores_duplicate_messages() {
    let agent = AgentWorker::new(Arc::new(MockLLMProvider::new("reply")), "system");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let mut router = ChannelRouter::new(agent);

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .clone()
        .expect("channel sender should be captured");
    let router_task = tokio::spawn(async move { router.run().await });
    let incoming = build_incoming();

    channel_tx
        .send(incoming.clone())
        .await
        .expect("first message should be sent");
    channel_tx
        .send(incoming)
        .await
        .expect("duplicate message should be sent");

    timeout(Duration::from_millis(500), async {
        loop {
            if sent.lock().await.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("outgoing message should be recorded");

    router_task.abort();

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].content, "reply");
    assert_eq!(recorded[0].metadata["event_kind"], "TextOutput");
    assert_eq!(recorded[0].metadata["session_channel"], "feishu");
    assert_eq!(recorded[0].metadata["session_user_id"], "ou_xxx");
    assert_eq!(
        recorded[0].metadata["session_external_thread_id"],
        "default"
    );
    assert_eq!(recorded[0].reply_to_message_id.as_deref(), Some("msg_1"));
}

#[tokio::test]
async fn router_stores_two_turns_for_same_session_thread_with_mock_agent() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let agent_handle = build_mock_agent_handle(Arc::clone(&observed_requests));
    let sessions = SessionManager::new();
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(agent_handle, sessions);

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let first_incoming = build_incoming_with(
        "msg_turn_1",
        "ou_shared",
        Some("thread_shared"),
        "first question",
    );
    let second_incoming = build_incoming_with(
        "msg_turn_2",
        "ou_shared",
        Some("thread_shared"),
        "second question",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(first_incoming)
            .await
            .expect("first message should be sent");
        channel_tx
            .send(second_incoming)
            .await
            .expect("second message should be sent");
    });

    router.run().await.expect("router loop should exit cleanly");
    send_task.await.expect("sender task should complete");

    timeout(Duration::from_millis(500), async {
        loop {
            if sent.lock().await.len() == 4 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all outgoing messages should be recorded");

    let observed_requests = observed_requests.lock().await.clone();
    let locator = observed_requests[0].locator.clone();
    let history = router.sessions().load_turn(&locator).await;
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_shared".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");
    let recorded = sent.lock().await.clone();

    assert_eq!(observed_requests.len(), 2);
    assert_eq!(observed_requests[0].incoming.content, "first question");
    assert_eq!(observed_requests[1].incoming.content, "second question");
    assert_eq!(observed_requests[0].locator.channel, "feishu");
    assert_eq!(observed_requests[0].locator.user_id, "ou_shared");
    assert_eq!(
        observed_requests[0].locator.external_thread_id,
        "thread_shared"
    );
    assert_eq!(observed_requests[1].locator.channel, "feishu");
    assert_eq!(observed_requests[1].locator.user_id, "ou_shared");
    assert_eq!(
        observed_requests[1].locator.external_thread_id,
        "thread_shared"
    );
    assert_eq!(
        observed_requests[0].locator.session_id,
        observed_requests[1].locator.session_id
    );
    assert_eq!(
        observed_requests[0].locator.thread_id,
        observed_requests[1].locator.thread_id
    );

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.external_thread_id, "thread_shared");
    assert_eq!(history.len(), 6);
    assert_eq!(thread.turns[0].messages.len(), 2);
    assert_eq!(thread.turns[1].messages.len(), 4);
    assert_eq!(thread.turns[0].messages[0].content, "first question");
    assert_eq!(thread.turns[0].messages[1].content, "reply-first");
    assert_eq!(thread.turns[1].messages[0].content, "second question");
    assert_eq!(thread.turns[1].messages[1].tool_calls[0].id, "call_mock_1");
    assert_eq!(
        thread.turns[1].messages[2].tool_call_id.as_deref(),
        Some("call_mock_1")
    );
    assert_eq!(thread.turns[1].messages[3].content, "reply-second");

    assert_eq!(recorded.len(), 4);
    assert_eq!(recorded[0].content, "reply-first");
    assert!(recorded[1].content.contains("[openjarvis][tool_call]"));
    assert!(recorded[2].content.contains("[openjarvis][tool_result]"));
    assert_eq!(recorded[3].content, "reply-second");
}

#[tokio::test]
async fn router_applies_five_message_truncation_strategy_before_next_turn() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let incoming_tx = Arc::new(Mutex::new(None));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let agent_handle = build_truncation_mock_agent_handle(Arc::clone(&observed_requests));
    let sessions = SessionManager::with_strategy(SessionStrategy {
        max_messages_per_turn: 5,
    });
    let mut router = ChannelRouter::with_session_manager_and_agent_handle(agent_handle, sessions);

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
            incoming_tx: Arc::clone(&incoming_tx),
        }))
        .await
        .expect("channel should register");

    let channel_tx = incoming_tx
        .lock()
        .await
        .take()
        .expect("channel sender should be captured");
    let first_incoming = build_incoming_with(
        "msg_truncation_1",
        "ou_truncation",
        Some("thread_truncation"),
        "trigger many replies",
    );
    let second_incoming = build_incoming_with(
        "msg_truncation_2",
        "ou_truncation",
        Some("thread_truncation"),
        "check history after truncation",
    );

    let send_task = tokio::spawn(async move {
        channel_tx
            .send(first_incoming)
            .await
            .expect("first truncation message should be sent");
        channel_tx
            .send(second_incoming)
            .await
            .expect("second truncation message should be sent");
    });

    router.run().await.expect("router loop should exit cleanly");
    send_task.await.expect("sender task should complete");

    timeout(Duration::from_millis(500), async {
        loop {
            if sent.lock().await.len() == 7 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all truncation outgoing messages should be recorded");

    let observed_requests = observed_requests.lock().await.clone();
    let locator = observed_requests[0].locator.clone();
    let history = router.sessions().load_turn(&locator).await;
    let session = router
        .sessions()
        .get_session(&SessionKey {
            channel: "feishu".to_string(),
            user_id: "ou_truncation".to_string(),
        })
        .await
        .expect("session should exist");
    let thread = session
        .threads
        .get(&locator.thread_id)
        .expect("thread should exist");
    let recorded = sent.lock().await.clone();

    assert_eq!(observed_requests.len(), 2);
    assert!(observed_requests[0].history.is_empty());
    assert_eq!(observed_requests[1].history.len(), 5);
    assert_eq!(
        observed_requests[1]
            .history
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "message_2".to_string(),
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );

    assert_eq!(thread.turns.len(), 2);
    assert_eq!(thread.external_thread_id, "thread_truncation");
    assert_eq!(thread.turns[0].messages.len(), 5);
    assert_eq!(
        thread.turns[0]
            .messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>(),
        vec![
            "message_2".to_string(),
            "message_3".to_string(),
            "message_4".to_string(),
            "message_5".to_string(),
            "message_6".to_string(),
        ]
    );
    assert_eq!(history.len(), 7);
    assert_eq!(history[0].content, "message_2");
    assert_eq!(history[4].content, "message_6");
    assert_eq!(history[5].content, "check history after truncation");
    assert_eq!(history[6].content, "final-after-truncation");

    assert_eq!(recorded.len(), 7);
    assert_eq!(recorded[0].content, "message_1");
    assert_eq!(recorded[5].content, "message_6");
    assert_eq!(recorded[6].content, "final-after-truncation");
}

fn build_mock_agent_handle(observed_requests: Arc<Mutex<Vec<AgentRequest>>>) -> AgentWorkerHandle {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);

    spawn_mock_agent_loop(observed_requests, event_tx, request_rx);

    AgentWorkerHandle {
        request_tx,
        event_rx,
    }
}

fn build_truncation_mock_agent_handle(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
) -> AgentWorkerHandle {
    let (request_tx, request_rx) = mpsc::channel(32);
    let (event_tx, event_rx) = mpsc::channel(32);

    spawn_truncation_mock_agent_loop(observed_requests, event_tx, request_rx);

    AgentWorkerHandle {
        request_tx,
        event_rx,
    }
}

fn spawn_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for step in 0..2 {
            let request = request_rx
                .recv()
                .await
                .expect("mock agent should receive scripted request");
            observed_requests.lock().await.push(request.clone());

            match step {
                0 => {
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "reply-first",
                            json!({
                                "source": "mock_agent",
                                "is_final": true,
                            }),
                            true,
                        )))
                        .await
                        .expect("first dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![ChatMessage::new(
                                ChatMessageRole::Assistant,
                                "reply-first",
                                Utc::now(),
                            )],
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("first completed turn should be sent");
                }
                1 => {
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::ToolCall,
                            "[openjarvis][tool_call] read {\"path\":\"Cargo.toml\"}",
                            json!({
                                "tool": "read",
                                "arguments": { "path": "Cargo.toml" },
                                "tool_call_id": "call_mock_1",
                            }),
                            true,
                        )))
                        .await
                        .expect("tool_call dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::ToolResult,
                            "[openjarvis][tool_result] ok",
                            json!({
                                "tool": "read",
                                "is_error": false,
                                "metadata": {},
                                "tool_call_id": "call_mock_1",
                            }),
                            false,
                        )))
                        .await
                        .expect("tool_result dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "reply-second",
                            json!({
                                "source": "mock_agent",
                                "is_final": true,
                            }),
                            false,
                        )))
                        .await
                        .expect("final text dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![
                                ChatMessage::new(ChatMessageRole::Assistant, "", Utc::now())
                                    .with_tool_calls(vec![ChatToolCall {
                                        id: "call_mock_1".to_string(),
                                        name: "read".to_string(),
                                        arguments: json!({ "path": "Cargo.toml" }),
                                    }]),
                                ChatMessage::new(ChatMessageRole::ToolResult, "ok", Utc::now())
                                    .with_tool_call_id("call_mock_1"),
                                ChatMessage::new(
                                    ChatMessageRole::Assistant,
                                    "reply-second",
                                    Utc::now(),
                                ),
                            ],
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("second completed turn should be sent");
                }
                _ => unreachable!("mock agent only scripts two requests"),
            }
        }
    })
}

fn spawn_truncation_mock_agent_loop(
    observed_requests: Arc<Mutex<Vec<AgentRequest>>>,
    event_tx: mpsc::Sender<AgentWorkerEvent>,
    mut request_rx: mpsc::Receiver<AgentRequest>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        for step in 0..2 {
            let request = request_rx
                .recv()
                .await
                .expect("truncation mock agent should receive scripted request");
            observed_requests.lock().await.push(request.clone());

            match step {
                0 => {
                    let mut turn_messages = Vec::new();
                    for index in 1..=6 {
                        let content = format!("message_{index}");
                        event_tx
                            .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                                &request,
                                AgentLoopEventKind::TextOutput,
                                &content,
                                json!({
                                    "source": "truncation_mock_agent",
                                    "message_index": index,
                                }),
                                index == 1,
                            )))
                            .await
                            .expect("truncation dispatch should be sent");
                        turn_messages.push(ChatMessage::new(
                            ChatMessageRole::Assistant,
                            content,
                            Utc::now(),
                        ));
                    }
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: turn_messages,
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("truncation completed turn should be sent");
                }
                1 => {
                    event_tx
                        .send(AgentWorkerEvent::Dispatch(build_dispatch_event(
                            &request,
                            AgentLoopEventKind::TextOutput,
                            "final-after-truncation",
                            json!({
                                "source": "truncation_mock_agent",
                                "is_final": true,
                            }),
                            true,
                        )))
                        .await
                        .expect("final truncation dispatch should be sent");
                    event_tx
                        .send(AgentWorkerEvent::TurnCompleted(CompletedAgentTurn {
                            locator: request.locator,
                            incoming: request.incoming,
                            messages: vec![ChatMessage::new(
                                ChatMessageRole::Assistant,
                                "final-after-truncation",
                                Utc::now(),
                            )],
                            completed_at: Utc::now(),
                        }))
                        .await
                        .expect("final truncation completed turn should be sent");
                }
                _ => unreachable!("truncation mock agent only scripts two requests"),
            }
        }
    })
}

fn build_dispatch_event(
    request: &AgentRequest,
    kind: AgentLoopEventKind,
    content: &str,
    metadata: serde_json::Value,
    reply_to_source: bool,
) -> AgentDispatchEvent {
    AgentDispatchEvent {
        kind,
        content: content.to_string(),
        metadata,
        channel: request.incoming.channel.clone(),
        thread_id: request.incoming.thread_id.clone(),
        source_message_id: request.incoming.external_message_id.clone(),
        target: request.incoming.reply_target.clone(),
        session_id: request.locator.session_id.to_string(),
        session_channel: request.locator.channel.clone(),
        session_user_id: request.locator.user_id.clone(),
        session_external_thread_id: request.locator.external_thread_id.clone(),
        session_thread_id: request.locator.thread_id.to_string(),
        reply_to_source,
    }
}

fn build_incoming() -> IncomingMessage {
    build_incoming_with("msg_1", "ou_xxx", None, "hello")
}

fn build_incoming_with(
    message_id: &str,
    user_id: &str,
    thread_id: Option<&str>,
    content: &str,
) -> IncomingMessage {
    IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some(message_id.to_string()),
        channel: "feishu".to_string(),
        user_id: user_id.to_string(),
        user_name: None,
        content: content.to_string(),
        thread_id: thread_id.map(|value| value.to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    }
}
