//! Shared inbound and outbound message models exchanged between channels and the router.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncomingAttachment {
    pub name: String,
    pub url: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyTarget {
    pub receive_id: String,
    pub receive_id_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    pub channel: String,
    pub user_id: String,
    pub user_name: Option<String>,
    pub content: String,
    pub thread_id: Option<String>,
    pub received_at: DateTime<Utc>,
    pub metadata: Value,
    pub attachments: Vec<IncomingAttachment>,
    pub reply_target: ReplyTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub id: Uuid,
    pub channel: String,
    pub content: String,
    pub thread_id: Option<String>,
    pub metadata: Value,
    pub reply_to_message_id: Option<String>,
    pub target: ReplyTarget,
}
