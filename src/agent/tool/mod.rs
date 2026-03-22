pub mod edit;
pub mod read;
pub mod shell;
pub mod write;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

pub use edit::EditTool;
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub content: String,
    pub metadata: Value,
    pub is_error: bool,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// 作用: 返回当前工具的定义信息，供 agent loop 和后续规划器使用。
    /// 参数: 无，返回工具名称和描述。
    fn definition(&self) -> ToolDefinition;

    /// 作用: 执行一次工具调用请求，并返回标准化结果。
    /// 参数: request 为工具名称和参数组成的调用请求。
    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult>;
}

#[derive(Default)]
pub struct ToolRegistry {
    handlers: RwLock<HashMap<String, Arc<dyn ToolHandler>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        // 作用: 创建一个空的工具注册表。
        // 参数: 无，默认不包含任何可调用工具。
        Self::default()
    }

    pub async fn register(&self, handler: Arc<dyn ToolHandler>) -> Result<()> {
        // 作用: 注册一个工具 handler，并以工具名称作为唯一键。
        // 参数: handler 为实现 ToolHandler trait 的工具实例。
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        if handlers.contains_key(&definition.name) {
            bail!("tool `{}` is already registered", definition.name);
        }

        handlers.insert(definition.name, handler);
        Ok(())
    }

    pub async fn register_builtin_tools(&self) -> Result<()> {
        // 作用: 一次性注册当前内置的四个基础文件与命令工具。
        // 参数: 无，当前会注册 read、write、edit 和 shell。
        self.register_if_missing(Arc::new(ReadTool::new())).await;
        self.register_if_missing(Arc::new(WriteTool::new())).await;
        self.register_if_missing(Arc::new(EditTool::new())).await;
        self.register_if_missing(Arc::new(ShellTool::new())).await;
        Ok(())
    }

    pub async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        // 作用: 根据工具名称查找已注册 handler 并执行调用。
        // 参数: request 为包含工具名和参数的标准化调用请求。
        let handler = {
            let handlers = self.handlers.read().await;
            handlers.get(&request.name).cloned()
        };
        let Some(handler) = handler else {
            bail!("tool `{}` is not registered", request.name);
        };

        handler.call(request).await
    }

    pub async fn list(&self) -> Vec<ToolDefinition> {
        // 作用: 返回当前全部已注册工具的定义列表。
        // 参数: 无，结果可用于后续向模型暴露工具能力。
        self.handlers
            .read()
            .await
            .values()
            .map(|handler| handler.definition())
            .collect()
    }

    async fn register_if_missing(&self, handler: Arc<dyn ToolHandler>) {
        // 作用: 在工具未注册时追加注册，已存在时保持静默跳过。
        // 参数: handler 为候选工具实例。
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        handlers.entry(definition.name).or_insert(handler);
    }
}

pub fn empty_tool_parameters_schema() -> Value {
    // 作用: 返回一个空对象参数 schema，用于暂时没有详细参数定义的工具。
    // 参数: 无，输出兼容 OpenAI function calling 的 JSON Schema 对象。
    json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false,
    })
}
