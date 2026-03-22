use openjarvis::agent::{EditTool, ToolCallRequest, ToolHandler};
use serde_json::json;
use std::{env::temp_dir, fs};
use uuid::Uuid;

#[tokio::test]
async fn edit_tool_replaces_matched_text() {
    let path = temp_dir().join(format!("openjarvis-edit-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello old world").expect("temp file should be written");

    let tool = EditTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "edit".to_string(),
            arguments: json!({
                "path": path,
                "old": "old",
                "new": "new",
            }),
        })
        .await
        .expect("edit tool should succeed");

    let updated = fs::read_to_string(path).expect("edited file should exist");
    assert_eq!(updated, "hello new world");
    assert!(result.content.contains("updated"));
    assert!(!result.is_error);
}
