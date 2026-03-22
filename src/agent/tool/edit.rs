use super::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fs;

#[derive(Default)]
pub struct EditTool;

#[derive(Debug, Deserialize)]
struct EditToolArguments {
    path: String,
    old: String,
    new: String,
    replace_all: Option<bool>,
}

impl EditTool {
    pub fn new() -> Self {
        // 作用: 创建内置 edit 工具实例。
        // 参数: 无，edit 工具用于基于字符串匹配修改文件内容。
        Self
    }
}

#[async_trait]
impl ToolHandler for EditTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit".to_string(),
            description: "Edit a text file by replacing matched text.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the text file that should be edited."
                    },
                    "old": {
                        "type": "string",
                        "description": "The existing text that should be replaced."
                    },
                    "new": {
                        "type": "string",
                        "description": "The replacement text."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Whether to replace all matches instead of a single match."
                    }
                },
                "required": ["path", "old", "new"],
                "additionalProperties": false,
            }),
        }
    }

    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        // 作用: 根据 old/new 规则编辑目标文件，并返回替换结果。
        // 参数: request.arguments 需要包含 path、old、new，replace_all 可选。
        let args: EditToolArguments =
            serde_json::from_value(request.arguments).context("invalid edit tool arguments")?;
        if args.old.is_empty() {
            bail!("edit tool requires non-empty `old` text");
        }

        let content = fs::read_to_string(&args.path)
            .with_context(|| format!("failed to read file {}", args.path))?;
        let match_count = content.matches(&args.old).count();
        if match_count == 0 {
            bail!("edit tool did not find target text in {}", args.path);
        }

        let replace_all = args.replace_all.unwrap_or(false);
        let updated = if replace_all {
            content.replace(&args.old, &args.new)
        } else {
            if match_count != 1 {
                bail!(
                    "edit tool expected exactly one match in {}, found {}",
                    args.path,
                    match_count
                );
            }
            content.replacen(&args.old, &args.new, 1)
        };

        fs::write(&args.path, &updated)
            .with_context(|| format!("failed to write file {}", args.path))?;

        Ok(ToolCallResult {
            content: format!(
                "updated {} occurrence(s)",
                if replace_all { match_count } else { 1 }
            ),
            metadata: json!({
                "path": args.path,
                "match_count": match_count,
                "replace_all": replace_all,
            }),
            is_error: false,
        })
    }
}
