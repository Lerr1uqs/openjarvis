use openjarvis::agent::{ToolCallRequest, ToolHandler, WriteTool};
use serde_json::json;
use std::{env::temp_dir, fs};
use uuid::Uuid;

#[tokio::test]
async fn write_tool_persists_file_contents() {
    let path = temp_dir().join(format!("openjarvis-write-{}.txt", Uuid::new_v4()));
    let tool = WriteTool::new();
    let payload = "hello from write tool";

    let result = tool
        .call(ToolCallRequest {
            name: "write".to_string(),
            arguments: json!({
                "path": path,
                "content": payload,
            }),
        })
        .await
        .expect("write tool should succeed");

    let written = fs::read_to_string(path).expect("written file should exist");
    assert_eq!(written, payload);
    assert!(result.content.contains("wrote"));
    assert!(!result.is_error);
}

#[tokio::test]
async fn write_tool_creates_parent_directories() {
    let root = temp_dir().join(format!("openjarvis-write-dir-{}", Uuid::new_v4()));
    let path = root.join("nested").join("file.txt");
    let tool = WriteTool::new();

    tool.call(ToolCallRequest {
        name: "write".to_string(),
        arguments: json!({
            "path": path,
            "content": "nested payload",
        }),
    })
    .await
    .expect("write tool should create parent directories");

    let written = fs::read_to_string(root.join("nested").join("file.txt"))
        .expect("written nested file should exist");
    assert_eq!(written, "nested payload");
}

#[tokio::test]
async fn write_tool_overwrites_existing_contents() {
    let path = temp_dir().join(format!("openjarvis-write-overwrite-{}.txt", Uuid::new_v4()));
    fs::write(&path, "old content").expect("temp file should be written");

    let tool = WriteTool::new();
    tool.call(ToolCallRequest {
        name: "write".to_string(),
        arguments: json!({
            "path": path,
            "content": "new content",
        }),
    })
    .await
    .expect("write tool should overwrite file");

    assert_eq!(
        fs::read_to_string(&path).expect("updated file should exist"),
        "new content"
    );
}

#[tokio::test]
async fn write_tool_supports_empty_content() {
    let path = temp_dir().join(format!("openjarvis-write-empty-{}.txt", Uuid::new_v4()));

    let tool = WriteTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "write".to_string(),
            arguments: json!({
                "path": path,
                "content": "",
            }),
        })
        .await
        .expect("write tool should support empty content");

    assert_eq!(
        fs::read_to_string(&path).expect("written file should exist"),
        ""
    );
    assert_eq!(result.metadata["bytes_written"], 0);
}

#[tokio::test]
async fn write_tool_rejects_unknown_arguments() {
    let path = temp_dir().join(format!("openjarvis-write-schema-{}.txt", Uuid::new_v4()));
    let tool = WriteTool::new();

    let error = tool
        .call(ToolCallRequest {
            name: "write".to_string(),
            arguments: json!({
                "path": path,
                "content": "hello",
                "append": true,
            }),
        })
        .await
        .expect_err("write tool should reject unknown arguments");

    assert!(format!("{error:#}").contains("unknown field"));
}

#[tokio::test]
async fn write_tool_rejects_missing_content() {
    let path = temp_dir().join(format!("openjarvis-write-missing-{}.txt", Uuid::new_v4()));
    let tool = WriteTool::new();

    let error = tool
        .call(ToolCallRequest {
            name: "write".to_string(),
            arguments: json!({
                "path": path,
            }),
        })
        .await
        .expect_err("write tool should reject missing content");

    assert!(format!("{error:#}").contains("missing field"));
}
