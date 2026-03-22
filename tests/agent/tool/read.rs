use openjarvis::agent::{ReadTool, ToolCallRequest, ToolHandler};
use serde_json::json;
use std::{env::temp_dir, fs};
use uuid::Uuid;

#[tokio::test]
async fn read_tool_returns_file_contents() {
    let path = temp_dir().join(format!("openjarvis-read-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello from read tool").expect("temp file should be written");

    let tool = ReadTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({ "path": path }),
        })
        .await
        .expect("read tool should succeed");

    assert_eq!(result.content, "hello from read tool");
    assert!(!result.is_error);
}
