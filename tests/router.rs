use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::AgentWorker,
    channels::{Channel, ChannelRegistration},
    llm::MockLLMProvider,
    model::{IncomingMessage, OutgoingMessage, ReplyTarget},
    router::ChannelRouter,
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    sync::Mutex,
    time::{Duration, timeout},
};
use uuid::Uuid;

struct RecordingChannel {
    name: &'static str,
    sent: Arc<Mutex<Vec<OutgoingMessage>>>,
}

#[async_trait]
impl Channel for RecordingChannel {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn start(self: Arc<Self>, mut registration: ChannelRegistration) -> Result<()> {
        let sent = Arc::clone(&self.sent);
        tokio::spawn(async move {
            if let Some(message) = registration.outgoing_rx.recv().await {
                sent.lock().await.push(message);
            }
        });
        Ok(())
    }
}

#[tokio::test]
async fn router_ignores_duplicate_messages() {
    let agent = AgentWorker::new(Arc::new(MockLLMProvider::new("reply")), "system");
    let router = ChannelRouter::new(agent);
    let sent = Arc::new(Mutex::new(Vec::new()));

    router
        .register_channel(Box::new(RecordingChannel {
            name: "feishu",
            sent: Arc::clone(&sent),
        }))
        .await
        .expect("channel should register");

    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: "hello".to_string(),
        thread_id: None,
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };

    router
        .handle_incoming(incoming.clone())
        .await
        .expect("first message should be processed");
    router
        .handle_incoming(incoming)
        .await
        .expect("duplicate message should be ignored");

    timeout(Duration::from_millis(300), async {
        loop {
            if sent.lock().await.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("outgoing message should be recorded");

    let recorded = sent.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].content, "reply");
}
