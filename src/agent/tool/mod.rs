//! Shared tool traits, schemas, and registry for the built-in agent tool set.

pub mod edit;
pub mod read;
pub mod shell;
pub mod write;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
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
    pub input_schema: ToolInputSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSchemaProtocol {
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInputSchema {
    json_schema: Value,
}

impl ToolInputSchema {
    /// Wrap a protocol-agnostic JSON Schema definition.
    pub fn new(json_schema: Value) -> Self {
        Self { json_schema }
    }

    /// Return the stored protocol-agnostic JSON Schema.
    pub fn json_schema(&self) -> &Value {
        &self.json_schema
    }

    /// Project the stored schema into the OpenAI tool schema shape.
    pub fn for_openai(&self) -> Value {
        self.json_schema.clone()
    }

    /// Project the stored schema into the Anthropic tool schema shape.
    pub fn for_anthropic(&self) -> Value {
        self.json_schema.clone()
    }

    /// Project the stored schema for the selected LLM protocol.
    pub fn for_protocol(&self, protocol: ToolSchemaProtocol) -> Value {
        match protocol {
            ToolSchemaProtocol::OpenAi => self.for_openai(),
            ToolSchemaProtocol::Anthropic => self.for_anthropic(),
        }
    }
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
    /// Return the definition exposed to the agent loop and the model provider.
    fn definition(&self) -> ToolDefinition;

    /// Execute one tool call and return a normalized result payload.
    async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult>;
}

#[derive(Default)]
pub struct ToolRegistry {
    // AGENT-TODO: 对String起别名
    handlers: RwLock<HashMap<String, Arc<dyn ToolHandler>>>,
}

pub fn tool_definition_from_args<T>(
    name: impl Into<String>,
    description: impl Into<String>,
) -> ToolDefinition
where
    T: JsonSchema,
{
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: tool_input_schema::<T>(),
    }
}

pub fn parse_tool_arguments<T>(request: ToolCallRequest, tool_name: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(request.arguments)
        .with_context(|| format!("invalid `{tool_name}` tool arguments"))
}

impl ToolRegistry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one tool handler by its unique tool name.
    pub async fn register(&self, handler: Arc<dyn ToolHandler>) -> Result<()> {
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        if handlers.contains_key(&definition.name) {
            bail!("tool `{}` is already registered", definition.name);
        }

        handlers.insert(definition.name, handler);
        Ok(())
    }

    pub async fn register_builtin_tools(&self) -> Result<()> {
        // Register the current built-in four-tool set.
        self.register_if_missing(Arc::new(ReadTool::new())).await;
        self.register_if_missing(Arc::new(WriteTool::new())).await;
        self.register_if_missing(Arc::new(EditTool::new())).await;
        self.register_if_missing(Arc::new(ShellTool::new())).await;
        Ok(())
    }

    /// Look up a registered tool and execute the request.
    pub async fn call(&self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let handler = self.handlers.read().await.get(&request.name).cloned();

        let Some(handler) = handler else {
            bail!("tool `{}` is not registered", request.name);
        };

        handler.call(request).await
    }

    /// Return all registered tool definitions.
    pub async fn list(&self) -> Vec<ToolDefinition> {
        self.handlers
            .read()
            .await
            .values()
            .map(|handler| handler.definition())
            .collect()
    }

    async fn register_if_missing(&self, handler: Arc<dyn ToolHandler>) {
        // Register the handler only when the name is not present yet.
        let definition = handler.definition();
        let mut handlers = self.handlers.write().await;
        handlers.entry(definition.name).or_insert(handler);
    }
}

/// Return an empty object schema for tools that currently do not accept any arguments.
pub fn empty_tool_input_schema() -> ToolInputSchema {
    ToolInputSchema::new(json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false,
    }))
}

pub fn tool_input_schema<T>() -> ToolInputSchema
where
    T: JsonSchema,
{
    let mut schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("tool input schema should serialize");
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    ToolInputSchema::new(schema)
}
