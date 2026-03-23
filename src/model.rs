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

impl IncomingMessage {
    /// Return the effective thread id used by router and session storage.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let message = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: None,
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// assert_eq!(message.resolved_thread_id(), "default");
    /// ```
    pub fn resolved_thread_id(&self) -> String {
        self.thread_id
            .clone()
            .filter(|thread_id| !thread_id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
            // UUID : 如果没有就创建
    }
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
