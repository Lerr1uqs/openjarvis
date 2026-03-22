use super::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fs;

#[derive(Default)]
pub struct ReadTool;

#[derive(Debug, Deserialize)]
struct ReadToolArguments {
    path: String,
}

impl ReadTool {
    pub fn new() -> Self {
        // 作用: 创建内置 read 工具实例。
        // 参数: 无，read 工具用于读取 UTF-8 文本文件内容。
        Self
    }
}

#[async_trait]
impl ToolHandler for ReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read".to_string(),
            description: "Read a UTF-8 text file from disk.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the UTF-8 text file to read."
                    }
                },
                "required": ["path"],
                "additionalProperties": false,
            }),
        }
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        // 作用: 读取指定路径上的 UTF-8 文本文件，并返回文件内容。
        // 参数: request.arguments 需要包含 path 字段，表示目标文件路径。
        let args: ReadToolArguments =
            serde_json::from_value(request.arguments).context("invalid read tool arguments")?;
        let content = fs::read_to_string(&args.path)
            .with_context(|| format!("failed to read file {}", args.path))?;

        Ok(ToolCallResult {
            content,
            metadata: json!({
                "path": args.path,
            }),
            is_error: false,
        })
    }
}
