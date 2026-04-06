//! Parse OpenJarvis attachment syntax markers from outgoing assistant text.

use crate::model::{OutgoingAttachment, OutgoingAttachmentKind, OutgoingMessage};
use std::path::Path;

const ATTACHMENT_PREFIX: &str = "#!openjarvis[";

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed attachment syntax output containing original text and extracted attachments.
pub struct AttachmentSyntaxParseResult {
    pub content: String,
    pub attachments: Vec<OutgoingAttachment>,
}

/// Parser for the `#!openjarvis[...]` outgoing attachment syntax.
pub struct AttachmentSyntaxParser;

impl AttachmentSyntaxParser {
    /// Parse attachment syntax markers from one plain-text assistant reply.
    ///
    /// The parser currently supports `#!openjarvis[image:/abs/path/to/file.png]`. It extracts
    /// structured attachments without altering the original text content.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     attachment_syntax::AttachmentSyntaxParser,
    ///     model::OutgoingAttachmentKind,
    /// };
    ///
    /// let parsed = AttachmentSyntaxParser::parse_content(
    ///     "截图如下\n#!openjarvis[image:/tmp/demo.png]",
    /// );
    ///
    /// assert_eq!(parsed.content, "截图如下\n#!openjarvis[image:/tmp/demo.png]");
    /// assert_eq!(parsed.attachments.len(), 1);
    /// assert_eq!(parsed.attachments[0].kind, OutgoingAttachmentKind::Image);
    /// assert_eq!(parsed.attachments[0].path, "/tmp/demo.png");
    /// ```
    pub fn parse_content(content: &str) -> AttachmentSyntaxParseResult {
        let mut attachments = Vec::new();
        let mut cursor = 0;

        while let Some(relative_start) = content[cursor..].find(ATTACHMENT_PREFIX) {
            let marker_start = cursor + relative_start;
            let payload_start = marker_start + ATTACHMENT_PREFIX.len();
            let Some(relative_end) = content[payload_start..].find(']') else {
                return AttachmentSyntaxParseResult {
                    content: content.to_string(),
                    attachments,
                };
            };
            let marker_end = payload_start + relative_end;
            let marker_body = &content[payload_start..marker_end];

            if let Some(attachment) = parse_attachment_marker(marker_body) {
                attachments.push(attachment);
            }

            cursor = marker_end + 1;
        }

        AttachmentSyntaxParseResult {
            content: content.to_string(),
            attachments,
        }
    }

    /// Parse attachment syntax markers and merge the discovered attachments into one outgoing
    /// message.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     attachment_syntax::AttachmentSyntaxParser,
    ///     model::{OutgoingMessage, ReplyTarget},
    /// };
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let message = OutgoingMessage {
    ///     id: Uuid::new_v4(),
    ///     channel: "feishu".to_string(),
    ///     content: "#!openjarvis[image:/tmp/demo.png]".to_string(),
    ///     external_thread_id: None,
    ///     metadata: json!({}),
    ///     reply_to_message_id: None,
    ///     attachments: Vec::new(),
    ///     target: ReplyTarget {
    ///         receive_id: "chat".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let parsed = AttachmentSyntaxParser::parse_message(message);
    /// assert_eq!(parsed.content, "#!openjarvis[image:/tmp/demo.png]");
    /// assert_eq!(parsed.attachments.len(), 1);
    /// ```
    pub fn parse_message(mut message: OutgoingMessage) -> OutgoingMessage {
        let parsed = Self::parse_content(&message.content);
        if parsed.attachments.is_empty() {
            return message;
        }

        message.content = parsed.content;
        message.attachments.extend(parsed.attachments);
        message
    }
}

fn parse_attachment_marker(marker_body: &str) -> Option<OutgoingAttachment> {
    let (kind, path) = marker_body.trim().split_once(':')?;
    let path = path.trim();
    if path.is_empty() || !Path::new(path).is_absolute() {
        return None;
    }

    match kind.trim() {
        "image" => Some(OutgoingAttachment {
            kind: OutgoingAttachmentKind::Image,
            path: path.to_string(),
            mime_type: None,
        }),
        _ => None,
    }
}
