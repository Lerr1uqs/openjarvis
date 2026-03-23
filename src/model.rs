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
    /// Upstream chat-software thread identifier.
    ///
    /// This field is owned by the external channel implementation such as Feishu or Telegram.
    /// It is only used to resolve the internal OpenJarvis thread identity and is not the real
    /// persisted conversation thread ID inside `SessionManager`.
    pub thread_id: Option<String>,
    pub received_at: DateTime<Utc>,
    pub metadata: Value,
    pub attachments: Vec<IncomingAttachment>,
    pub reply_target: ReplyTarget,
}

impl IncomingMessage {
    /// Return the normalized external thread id reported by the upstream chat software.
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
    /// assert_eq!(message.resolved_external_thread_id(), "default");
    /// ```
    pub fn resolved_external_thread_id(&self) -> String {
        self.thread_id
            .clone()
            .filter(|thread_id| !thread_id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Return the normalized external thread id.
    ///
    /// This compatibility helper keeps older call sites working while the internal thread
    /// identity is resolved by the session layer.
    pub fn resolved_thread_id(&self) -> String {
        self.resolved_external_thread_id()
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
