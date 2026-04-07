//! LLM provider abstraction and OpenAI-compatible request/response serialization.

use crate::{
    agent::{ToolDefinition, ToolSchemaProtocol},
    config::{LLMConfig, global_config},
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
    time::Instant,
};
use tracing::debug;

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

/// Build one LLM provider directly from the installed global app config snapshot.
///
/// # 示例
/// ```rust,no_run
/// use openjarvis::config::{AppConfig, install_global_config};
/// use openjarvis::llm::build_provider_from_global_config;
///
/// let config = AppConfig::builder_for_test().build().expect("config should build");
/// install_global_config(config).expect("config should install");
///
/// let _provider = build_provider_from_global_config().expect("provider should build");
/// ```
pub fn build_provider_from_global_config() -> Result<Arc<dyn LLMProvider>> {
    build_provider(global_config().llm_config())
}

enum LLMProviderProtocol {
    Mock,
    OpenAiCompatible,
    Anthropic,
}

impl LLMProviderProtocol {
    fn from_config(config: &LLMConfig) -> Result<Self> {
        // Resolve the protocol from config so provider names can stay vendor-specific.
        match config.effective_protocol() {
            "mock" => Ok(Self::Mock),
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            "anthropic" => Ok(Self::Anthropic),
            other => bail!("unsupported llm protocol `{other}`"),
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
            bail!("llm.api_key is required when llm.protocol=openai_compatible");
        }
        if config.model.trim().is_empty() {
            bail!("llm.model is required when llm.protocol=openai_compatible");
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

        let message_count = request.messages.len();
        let tool_count = request.tools.len();
        let started_at = Instant::now();
        debug!(
            protocol = self.config.effective_protocol(),
            provider = %self.config.provider,
            model = %self.config.model,
            base_url = %self.config.base_url,
            message_count,
            tool_count,
            "starting llm network request"
        );
        let openai_request = build_openai_request(&self.config, request)?;
        let response = match self.client.chat().create(openai_request).await {
            Ok(response) => response,
            Err(error) => {
                debug!(
                    protocol = self.config.effective_protocol(),
                    provider = %self.config.provider,
                    model = %self.config.model,
                    base_url = %self.config.base_url,
                    message_count,
                    tool_count,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "llm network request failed"
                );
                return Err(error).with_context(|| {
                    format!(
                        "failed to call llm provider `{}` model `{}` at `{}`",
                        self.config.provider, self.config.model, self.config.base_url
                    )
                });
            }
        };
        let usage = response.usage.clone();
        let cached_tokens = usage
            .as_ref()
            .and_then(|usage| usage.prompt_tokens_details.as_ref())
            .and_then(|details| details.cached_tokens)
            .unwrap_or_default();
        debug!(
            protocol = self.config.effective_protocol(),
            provider = %self.config.provider,
            model = %self.config.model,
            base_url = %self.config.base_url,
            message_count,
            tool_count,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            choice_count = response.choices.len(),
            prompt_tokens = usage.as_ref().map(|usage| usage.prompt_tokens).unwrap_or_default(),
            completion_tokens = usage
                .as_ref()
                .map(|usage| usage.completion_tokens)
                .unwrap_or_default(),
            total_tokens = usage.as_ref().map(|usage| usage.total_tokens).unwrap_or_default(),
            cached_tokens,
            "completed llm network request"
        );
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
    let mut serialized = Vec::with_capacity(messages.len());
    let mut cursor = 0usize;
    while cursor < messages.len() {
        let message = &messages[cursor];
        match message.role {
            ChatMessageRole::Toolcall => {
                let (tool_call_message, consumed) =
                    collect_tool_call_messages(messages, cursor, None);
                serialized.push(serialize_assistant_message(&tool_call_message)?.into());
                cursor += consumed;
            }
            ChatMessageRole::Assistant if message_starts_tool_call_batch(messages, cursor) => {
                let (assistant_message, consumed) =
                    collect_tool_call_messages(messages, cursor + 1, Some(message));
                serialized.push(serialize_assistant_message(&assistant_message)?.into());
                cursor += consumed + 1;
            }
            _ => {
                serialized.push(serialize_context_message(message)?);
                cursor += 1;
            }
        }
    }

    Ok(serialized)
}

fn serialize_context_message(message: &ChatMessage) -> Result<ChatCompletionRequestMessage> {
    // Convert one unified message into the matching OpenAI message variant.
    match message.role {
        ChatMessageRole::System => Ok(ChatCompletionRequestSystemMessageArgs::default()
            .content(message.content.clone())
            .build()
            .context("failed to build system message")?
            .into()),
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
            Ok(serialize_assistant_message(message)?.into())
        }
    }
}

fn serialize_assistant_message(
    message: &ChatMessage,
) -> Result<async_openai::types::chat::ChatCompletionRequestAssistantMessage> {
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

    builder.build().context("failed to build assistant message")
}

fn message_starts_tool_call_batch(messages: &[ChatMessage], cursor: usize) -> bool {
    let message = &messages[cursor];
    if !message.tool_calls.is_empty() {
        return true;
    }

    messages
        .get(cursor + 1)
        .map(|next| next.role == ChatMessageRole::Toolcall)
        .unwrap_or(false)
}

fn collect_tool_call_messages(
    messages: &[ChatMessage],
    start: usize,
    assistant_message: Option<&ChatMessage>,
) -> (ChatMessage, usize) {
    let mut tool_calls = assistant_message
        .map(|message| message.tool_calls.clone())
        .unwrap_or_default();
    let created_at = assistant_message
        .map(|message| message.created_at)
        .or_else(|| messages.get(start).map(|message| message.created_at))
        .unwrap_or_else(chrono::Utc::now);
    let content = assistant_message
        .map(|message| message.content.clone())
        .unwrap_or_default();
    let mut consumed = 0usize;

    while let Some(message) = messages.get(start + consumed) {
        if message.role != ChatMessageRole::Toolcall {
            break;
        }
        tool_calls.extend(message.tool_calls.clone());
        consumed += 1;
    }

    (
        ChatMessage::new(ChatMessageRole::Assistant, content, created_at)
            .with_tool_calls(tool_calls),
        consumed,
    )
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

#[cfg(test)]
mod tests {
    use super::serialize_openai_messages;
    use crate::context::{ChatMessage, ChatMessageRole, ChatToolCall};
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn serialize_openai_messages_merges_split_toolcall_messages() {
        // 测试场景: Thread 正式消息按 Toolcall 拆开后，发给 OpenAI 时仍要还原成单个 assistant tool_calls payload。
        let now = Utc::now();
        let serialized = serialize_openai_messages(&[
            ChatMessage::new(ChatMessageRole::System, "system", now),
            ChatMessage::new(ChatMessageRole::User, "run tools", now),
            ChatMessage::new(ChatMessageRole::Assistant, "我先执行两个工具", now),
            ChatMessage::new(ChatMessageRole::Toolcall, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "Cargo.toml" }),
                },
            ]),
            ChatMessage::new(ChatMessageRole::Toolcall, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_2".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "src/main.rs" }),
                },
            ]),
            ChatMessage::new(ChatMessageRole::ToolResult, "ok", now).with_tool_call_id("call_1"),
            ChatMessage::new(ChatMessageRole::ToolResult, "ok", now).with_tool_call_id("call_2"),
        ])
        .expect("messages should serialize");
        let serialized_json = serialized
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized openai messages should convert to json");

        assert_eq!(serialized_json.len(), 5);
        assert_eq!(serialized_json[2]["role"], "assistant");
        assert_eq!(serialized_json[2]["content"], "我先执行两个工具");
        assert_eq!(
            serialized_json[2]["tool_calls"].as_array().unwrap().len(),
            2
        );
        assert_eq!(serialized_json[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(serialized_json[2]["tool_calls"][1]["id"], "call_2");
    }
}
