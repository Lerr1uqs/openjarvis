use super::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path};

#[derive(Default)]
pub struct WriteTool;

#[derive(Debug, Deserialize)]
struct WriteToolArguments {
    path: String,
    content: String,
}

impl WriteTool {
    pub fn new() -> Self {
        // 作用: 创建内置 write 工具实例。
        // 参数: 无，write 工具用于覆盖写入文本文件。
        Self
    }
}

#[async_trait]
impl ToolHandler for WriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write".to_string(),
            description: "Write full UTF-8 text content to a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file that should be written."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full UTF-8 text content to write into the file."
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false,
            }),
        }
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        // 作用: 把文本完整写入指定文件，必要时自动创建父目录。
        // 参数: request.arguments 需要包含 path 和 content 字段。
        let args: WriteToolArguments =
            serde_json::from_value(request.arguments).context("invalid write tool arguments")?;
        if let Some(parent) = Path::new(&args.path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directories for {}", args.path)
                })?;
            }
        }

        fs::write(&args.path, &args.content)
            .with_context(|| format!("failed to write file {}", args.path))?;

        Ok(ToolCallResult {
            content: format!("wrote {} bytes", args.content.len()),
            metadata: json!({
                "path": args.path,
                "bytes_written": args.content.len(),
            }),
            is_error: false,
        })
    }
}
