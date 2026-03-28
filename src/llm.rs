//! LLM provider abstraction and OpenAI-compatible request/response serialization.

use crate::{
    agent::{ToolDefinition, ToolSchemaProtocol},
    config::LLMConfig,
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
};
use anyhow::{Context, Result, bail};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionToolChoiceOption,
        ChatCompletionTools, CreateChatCompletionRequest, CreateChatCompletionRequestArgs,
        FunctionCall, FunctionObjectArgs, ToolChoiceOptions,
    },
};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct LLMRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
}

pub type LLMToolCall = ChatToolCall;

#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub message: Option<ChatMessage>,
    pub tool_calls: Vec<LLMToolCall>,
}

#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Generate one model response from structured messages and tool definitions.
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse>;
}

/// Build the configured LLM provider implementation.
pub fn build_provider(config: &LLMConfig) -> Result<Arc<dyn LLMProvider>> {
    match LLMProviderProtocol::from_config(config)? {
        LLMProviderProtocol::Mock => {
            Ok(Arc::new(MockLLMProvider::new(config.mock_response.clone())))
        }
        LLMProviderProtocol::OpenAiCompatible => {
            let resolved_config = resolve_provider_config(config)?;
            Ok(Arc::new(OpenaiProvider::new(resolved_config)?))
        }
        LLMProviderProtocol::Anthropic => Ok(Arc::new(AnthropicProvider::new(
            resolve_provider_config(config)?,
        ))),
    }
}

enum LLMProviderProtocol {
    Mock,
    OpenAiCompatible,
    Anthropic,
}

impl LLMProviderProtocol {
    fn from_config(config: &LLMConfig) -> Result<Self> {
        // Normalize configured provider aliases into one internal protocol enum.
        // TODO: 这里的check有问题 Provider很多 应该限制协议
        match config.provider.trim().to_ascii_lowercase().as_str() {
            "mock" | "mock_llm" => Ok(Self::Mock),
            "openai_compatible" | "openai" | "deepseek" | "ark" => Ok(Self::OpenAiCompatible),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            other => bail!("unsupported llm provider `{other}`"),
        }
    }
}

pub struct MockLLMProvider {
    response: String,
}

impl MockLLMProvider {
    /// Create a fixed-response mock provider for local loop tests.
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

#[async_trait]
impl LLMProvider for MockLLMProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        // Return a deterministic assistant reply without calling any external service.
        Ok(LLMResponse {
            message: Some(ChatMessage::new(
                ChatMessageRole::Assistant,
                self.response.clone(),
                chrono::Utc::now(),
            )),
            tool_calls: Vec::new(),
        })
    }
}

pub struct OpenaiProvider {
    config: LLMConfig,
    client: Client<OpenAIConfig>,
}

impl OpenaiProvider {
    fn new(config: LLMConfig) -> Result<Self> {
        // Validate the required fields and build an OpenAI-compatible client.
        if config.api_key.trim().is_empty() {
            bail!("llm.api_key is required when provider=openai_compatible");
        }
        if config.model.trim().is_empty() {
            bail!("llm.model is required when provider=openai_compatible");
        }

        let client_config = OpenAIConfig::new()
            .with_api_key(config.api_key.clone())
            .with_api_base(config.base_url.clone());

        Ok(Self {
            config,
            client: Client::with_config(client_config),
        })
    }
}

pub struct AnthropicProvider {
    config: LLMConfig,
}

impl AnthropicProvider {
    fn new(config: LLMConfig) -> Self {
        // Keep a dedicated protocol branch even before the Anthropic transport is implemented.
        Self { config }
    }
}

#[async_trait]
impl LLMProvider for OpenaiProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        // Execute one OpenAI-compatible chat completion request and normalize the reply.
        if request.messages.is_empty() {
            bail!("llm request must contain at least one message");
        }

        let response = self
            .client
            .chat()
            .create(build_openai_request(&self.config, request)?)
            .await
            .with_context(|| {
                format!(
                    "failed to call llm provider `{}` model `{}` at `{}`",
                    self.config.provider, self.config.model, self.config.base_url
                )
            })?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .context("llm response did not contain any choices")?;
        let message = choice.message;
        let assistant_message = message
            .content
            .filter(|content| !content.trim().is_empty())
            .map(|content| {
                ChatMessage::new(ChatMessageRole::Assistant, content, chrono::Utc::now())
            });
        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(parse_openai_tool_call)
            .collect::<Result<Vec<_>>>()?;

        Ok(LLMResponse {
            message: assistant_message,
            tool_calls,
        })
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        // Keep the protocol-specific entrypoint in place until the transport is implemented.
        let _ = build_anthropic_request(&self.config, &request)?;
        unreachable!("anthropic placeholder should always return an error before this point");
    }
}

fn build_openai_request(
    config: &LLMConfig,
    request: LLMRequest,
) -> Result<CreateChatCompletionRequest> {
    // Convert the protocol-agnostic request into the OpenAI SDK request shape.
    let messages = serialize_openai_messages(&request.messages)?;
    let tools = serialize_openai_tools(&request.tools)?;
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(config.model.clone());
    builder.messages(messages);
    builder.temperature(0.1);
    #[allow(deprecated)]
    {
        builder.max_tokens(u32::try_from(config.max_output_tokens()).unwrap_or(u32::MAX));
    }

    if !tools.is_empty() {
        builder.tools(tools);
        builder.tool_choice(ChatCompletionToolChoiceOption::Mode(
            ToolChoiceOptions::Auto,
        ));
    }

    builder
        .build()
        .context("failed to build openai chat completion request")
}

fn serialize_openai_messages(
    messages: &[ChatMessage],
) -> Result<Vec<ChatCompletionRequestMessage>> {
    // Serialize the unified chat history into OpenAI chat-completion messages.
    messages
        .iter()
        .map(serialize_context_message)
        .collect::<Result<Vec<_>>>()
}

fn serialize_context_message(message: &ChatMessage) -> Result<ChatCompletionRequestMessage> {
    // Convert one unified message into the matching OpenAI message variant.
    match message.role {
        ChatMessageRole::System | ChatMessageRole::Memory => {
            Ok(ChatCompletionRequestSystemMessageArgs::default()
                .content(message.content.clone())
                .build()
                .context("failed to build system message")?
                .into())
        }
        ChatMessageRole::User => Ok(ChatCompletionRequestUserMessageArgs::default()
            .content(message.content.clone())
            .build()
            .context("failed to build user message")?
            .into()),
        ChatMessageRole::ToolResult => Ok(ChatCompletionRequestToolMessageArgs::default()
            .content(message.content.clone())
            .tool_call_id(
                message
                    .tool_call_id
                    .clone()
                    .context("tool result message is missing tool_call_id")?,
            )
            .build()
            .context("failed to build tool result message from context")?
            .into()),
        ChatMessageRole::Assistant | ChatMessageRole::Toolcall => {
            let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
            if !message.content.trim().is_empty() {
                builder.content(message.content.clone());
            }
            if !message.tool_calls.is_empty() {
                builder.tool_calls(
                    message
                        .tool_calls
                        .iter()
                        .map(serialize_openai_tool_call)
                        .collect::<Vec<_>>(),
                );
            }

            Ok(builder
                .build()
                .context("failed to build assistant message")?
                .into())
        }
    }
}

fn serialize_openai_tools(tools: &[ToolDefinition]) -> Result<Vec<ChatCompletionTools>> {
    // Project unified tool definitions into OpenAI-compatible tool descriptors.
    tools
        .iter()
        .map(|tool| {
            let function = FunctionObjectArgs::default()
                .name(tool.name.clone())
                .description(tool.description.clone())
                .parameters(tool.input_schema.for_protocol(ToolSchemaProtocol::OpenAi))
                .build()
                .with_context(|| format!("failed to build tool schema for `{}`", tool.name))?;
            Ok(ChatCompletionTools::Function(ChatCompletionTool {
                function,
            }))
        })
        .collect()
}

fn build_anthropic_request(config: &LLMConfig, request: &LLMRequest) -> Result<()> {
    // Keep Anthropic request assembly behind its own protocol boundary for future implementation.
    if request.messages.is_empty() {
        bail!("llm request must contain at least one message");
    }

    let _ = serialize_anthropic_messages(&request.messages)?;
    let _ = serialize_anthropic_tools(&request.tools)?;
    let _ = config;
    bail!("provider protocol `anthropic` is not implemented yet")
}

fn serialize_anthropic_messages(messages: &[ChatMessage]) -> Result<Vec<Value>> {
    // Placeholder serializer for Anthropic-compatible message payloads.
    let serialized = messages
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": format!("{:?}", message.role).to_ascii_lowercase(),
                "content": message.content.clone(),
                "tool_call_id": message.tool_call_id.clone(),
            })
        })
        .collect::<Vec<_>>();
    Ok(serialized)
}

fn serialize_anthropic_tools(tools: &[ToolDefinition]) -> Result<Vec<Value>> {
    // Placeholder serializer for Anthropic-compatible tool descriptors.
    Ok(tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name.clone(),
                "description": tool.description.clone(),
                "input_schema": tool.input_schema.for_protocol(ToolSchemaProtocol::Anthropic),
            })
        })
        .collect())
}

fn serialize_openai_tool_call(tool_call: &LLMToolCall) -> ChatCompletionMessageToolCalls {
    // Serialize one normalized tool call back into the OpenAI assistant-tool-call shape.
    ChatCompletionMessageToolCall {
        id: tool_call.id.clone(),
        function: FunctionCall {
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.to_string(),
        },
    }
    .into()
}

fn parse_openai_tool_call(tool_call: ChatCompletionMessageToolCalls) -> Result<LLMToolCall> {
    // Parse one OpenAI SDK tool call into the unified tool call model.
    match tool_call {
        ChatCompletionMessageToolCalls::Function(tool_call) => {
            let arguments = serde_json::from_str::<Value>(&tool_call.function.arguments)
                .with_context(|| {
                    format!(
                        "failed to parse tool arguments for `{}`",
                        tool_call.function.name
                    )
                })?;
            Ok(LLMToolCall {
                id: tool_call.id,
                name: tool_call.function.name,
                arguments,
            })
        }
        ChatCompletionMessageToolCalls::Custom(tool_call) => bail!(
            "custom tool calls are not supported yet: `{}`",
            tool_call.custom_tool.name
        ),
    }
}

fn resolve_provider_config(config: &LLMConfig) -> Result<LLMConfig> {
    // Resolve the final config, loading the API key from `api_key_path` when needed.
    let mut resolved = config.clone();
    if resolved.api_key.trim().is_empty() && !resolved.api_key_path.as_os_str().is_empty() {
        let expanded_path = expand_home_dir(&resolved.api_key_path)?;
        resolved.api_key = fs::read_to_string(&expanded_path)
            .with_context(|| format!("failed to read api key from {}", expanded_path.display()))?
            .trim()
            .to_string();
    }

    Ok(resolved)
}

fn expand_home_dir(path: &Path) -> Result<PathBuf> {
    // Expand `~` prefixes so API key paths can be resolved consistently across environments.
    let raw = path.to_string_lossy();
    if raw == "~" {
        return resolve_home_dir();
    }
    if let Some(suffix) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return Ok(resolve_home_dir()?.join(suffix));
    }

    Ok(path.to_path_buf())
}

fn resolve_home_dir() -> Result<PathBuf> {
    // Resolve the current user's home directory for path expansion.
    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    if let Ok(user_profile) = env::var("USERPROFILE") {
        if !user_profile.trim().is_empty() {
            return Ok(PathBuf::from(user_profile));
        }
    }

    bail!("failed to resolve user home directory for api_key_path")
}
