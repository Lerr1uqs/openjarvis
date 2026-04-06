use openjarvis::{
    attachment_syntax::AttachmentSyntaxParser,
    model::{OutgoingAttachmentKind, OutgoingMessage, ReplyTarget},
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn parser_extracts_image_attachments_from_content() {
    // 测试场景: 合法的 image 语法应被提取为结构化附件，同时原始文本必须完整保留。
    let parsed = AttachmentSyntaxParser::parse_content(
        "这是图片\n#!openjarvis[image:/tmp/demo.png]\n请查看",
    );

    assert_eq!(
        parsed.content,
        "这是图片\n#!openjarvis[image:/tmp/demo.png]\n请查看"
    );
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].kind, OutgoingAttachmentKind::Image);
    assert_eq!(parsed.attachments[0].path, "/tmp/demo.png");
}

#[test]
fn parser_ignores_invalid_or_relative_attachment_markers() {
    // 测试场景: 不支持的语法和相对路径不能污染结构化附件，原文本要完整保留。
    let parsed = AttachmentSyntaxParser::parse_content(concat!(
        "#!openjarvis[file:/tmp/demo.txt]\n",
        "#!openjarvis[image:relative/demo.png]\n",
        "#!openjarvis[image:]"
    ));

    assert!(parsed.attachments.is_empty());
    assert_eq!(
        parsed.content,
        concat!(
            "#!openjarvis[file:/tmp/demo.txt]\n",
            "#!openjarvis[image:relative/demo.png]\n",
            "#!openjarvis[image:]"
        )
    );
}

#[test]
fn parser_merges_extracted_attachments_into_outgoing_message() {
    // 测试场景: router 解析消息时应直接把语法提取进 OutgoingMessage.attachments，但不能改写原始文本。
    let parsed = AttachmentSyntaxParser::parse_message(OutgoingMessage {
        id: Uuid::new_v4(),
        channel: "feishu".to_string(),
        content: "#!openjarvis[image:/tmp/demo.png]".to_string(),
        external_thread_id: None,
        metadata: json!({}),
        reply_to_message_id: None,
        attachments: Vec::new(),
        target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    });

    assert_eq!(parsed.content, "#!openjarvis[image:/tmp/demo.png]");
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].path, "/tmp/demo.png");
}
