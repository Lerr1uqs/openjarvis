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
                "old_text": "old",
                "new_text": "new",
            }),
        })
        .await
        .expect("edit tool should succeed");

    let updated = fs::read_to_string(path).expect("edited file should exist");
    assert_eq!(updated, "hello new world");
    assert!(result.content.contains("updated"));
    assert!(!result.is_error);
}

#[tokio::test]
async fn edit_tool_replaces_only_first_match_when_multiple_exist() {
    let path = temp_dir().join(format!("openjarvis-edit-multi-{}.txt", Uuid::new_v4()));
    fs::write(&path, "old old old").expect("temp file should be written");

    let tool = EditTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "edit".to_string(),
            arguments: json!({
                "path": path,
                "old_text": "old",
                "new_text": "new",
            }),
        })
        .await
        .expect("edit tool should replace the first match");

    let updated = fs::read_to_string(&path).expect("edited file should exist");
    assert_eq!(updated, "new old old");
    assert_eq!(result.metadata["match_count"], 3);
    assert_eq!(result.metadata["replaced_count"], 1);
}

#[tokio::test]
async fn edit_tool_supports_deleting_text() {
    let path = temp_dir().join(format!("openjarvis-edit-delete-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello cruft world").expect("temp file should be written");

    let tool = EditTool::new();
    tool.call(ToolCallRequest {
        name: "edit".to_string(),
        arguments: json!({
            "path": path,
            "old_text": "cruft ",
            "new_text": "",
        }),
    })
    .await
    .expect("edit tool should allow deletion");

    let updated = fs::read_to_string(&path).expect("edited file should exist");
    assert_eq!(updated, "hello world");
}

#[tokio::test]
async fn edit_tool_supports_legacy_old_new_aliases() {
    let path = temp_dir().join(format!("openjarvis-edit-alias-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello old world").expect("temp file should be written");

    let tool = EditTool::new();
    tool.call(ToolCallRequest {
        name: "edit".to_string(),
        arguments: json!({
            "path": path,
            "old": "old",
            "new": "new",
        }),
    })
    .await
    .expect("edit tool should accept legacy field aliases");

    let updated = fs::read_to_string(&path).expect("edited file should exist");
    assert_eq!(updated, "hello new world");
}

#[tokio::test]
async fn edit_tool_rejects_empty_old_text() {
    let path = temp_dir().join(format!("openjarvis-edit-empty-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello world").expect("temp file should be written");

    let tool = EditTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "edit".to_string(),
            arguments: json!({
                "path": path,
                "old_text": "",
                "new_text": "new",
            }),
        })
        .await
        .expect_err("edit tool should reject empty old_text");

    assert!(error.to_string().contains("non-empty `old_text`"));
}

#[tokio::test]
async fn edit_tool_rejects_missing_target_text() {
    let path = temp_dir().join(format!("openjarvis-edit-missing-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello world").expect("temp file should be written");

    let tool = EditTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "edit".to_string(),
            arguments: json!({
                "path": path,
                "old_text": "missing",
                "new_text": "new",
            }),
        })
        .await
        .expect_err("edit tool should reject missing target text");

    assert!(error.to_string().contains("did not find target text"));
}

#[tokio::test]
async fn edit_tool_rejects_unknown_arguments() {
    let path = temp_dir().join(format!("openjarvis-edit-schema-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello world").expect("temp file should be written");

    let tool = EditTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "edit".to_string(),
            arguments: json!({
                "path": path,
                "old_text": "hello",
                "new_text": "hi",
                "replace_all": true,
            }),
        })
        .await
        .expect_err("edit tool should reject unknown arguments");

    assert!(format!("{error:#}").contains("unknown field"));
}
