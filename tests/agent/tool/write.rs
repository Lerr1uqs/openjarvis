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
