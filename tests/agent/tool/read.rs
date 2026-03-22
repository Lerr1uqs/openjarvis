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

#[tokio::test]
async fn read_tool_supports_line_range() {
    let path = temp_dir().join(format!("openjarvis-read-lines-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\nline-3\n").expect("temp file should be written");

    let tool = ReadTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 2,
                "end_line": 3,
            }),
        })
        .await
        .expect("read tool should succeed");

    assert_eq!(result.content, "line-2\nline-3\n");
    assert!(!result.is_error);
}

#[tokio::test]
async fn read_tool_supports_start_line_without_end_line() {
    let path = temp_dir().join(format!("openjarvis-read-start-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\nline-3").expect("temp file should be written");

    let tool = ReadTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 2,
            }),
        })
        .await
        .expect("read tool should succeed");

    assert_eq!(result.content, "line-2\nline-3");
}

#[tokio::test]
async fn read_tool_supports_end_line_without_start_line() {
    let path = temp_dir().join(format!("openjarvis-read-end-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\nline-3").expect("temp file should be written");

    let tool = ReadTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "end_line": 2,
            }),
        })
        .await
        .expect("read tool should succeed");

    assert_eq!(result.content, "line-1\nline-2\n");
}

#[tokio::test]
async fn read_tool_rejects_zero_based_line_numbers() {
    let path = temp_dir().join(format!("openjarvis-read-zero-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\n").expect("temp file should be written");

    let tool = ReadTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 0,
            }),
        })
        .await
        .expect_err("read tool should reject zero-based line numbers");

    assert!(error.to_string().contains("1-based"));
}

#[tokio::test]
async fn read_tool_rejects_reversed_ranges() {
    let path = temp_dir().join(format!("openjarvis-read-range-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\n").expect("temp file should be written");

    let tool = ReadTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 3,
                "end_line": 2,
            }),
        })
        .await
        .expect_err("read tool should reject reversed ranges");

    assert!(error.to_string().contains("start_line <= end_line"));
}

#[tokio::test]
async fn read_tool_rejects_out_of_range_start_line() {
    let path = temp_dir().join(format!("openjarvis-read-oob-{}.txt", Uuid::new_v4()));
    fs::write(&path, "line-1\nline-2\n").expect("temp file should be written");

    let tool = ReadTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 9,
            }),
        })
        .await
        .expect_err("read tool should reject out-of-range start lines");

    assert!(error.to_string().contains("out of range"));
}

#[tokio::test]
async fn read_tool_allows_empty_file_for_first_line_range() {
    let path = temp_dir().join(format!("openjarvis-read-empty-{}.txt", Uuid::new_v4()));
    fs::write(&path, "").expect("temp file should be written");

    let tool = ReadTool::new();
    let result = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "start_line": 1,
                "end_line": 1,
            }),
        })
        .await
        .expect("empty file with first-line range should succeed");

    assert_eq!(result.content, "");
}

#[tokio::test]
async fn read_tool_rejects_unknown_arguments() {
    let path = temp_dir().join(format!("openjarvis-read-schema-{}.txt", Uuid::new_v4()));
    fs::write(&path, "hello").expect("temp file should be written");

    let tool = ReadTool::new();
    let error = tool
        .call(ToolCallRequest {
            name: "read".to_string(),
            arguments: json!({
                "path": path,
                "unknown": true,
            }),
        })
        .await
        .expect_err("read tool should reject unknown arguments");

    assert!(format!("{error:#}").contains("unknown field"));
}
