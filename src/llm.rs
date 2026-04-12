//! LLM provider abstraction and protocol adapters for OpenAI chat completions, OpenAI Responses,
//! and Anthropic-compatible payloads.

use crate::{
    agent::{ToolDefinition, ToolSchemaProtocol},
    config::{LLMConfig, ResolvedLLMProviderConfig, global_config},
    context::{ChatMessage, ChatMessageRole, ChatToolCall},
};
use anyhow::{Context, Result, bail};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        chat::{
            ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
            ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
            ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
            ChatCompletionRequestUserMessageArgs, ChatCompletionTool,
            ChatCompletionToolChoiceOption, ChatCompletionTools, CreateChatCompletionRequest,
            CreateChatCompletionRequestArgs, FunctionCall, FunctionObjectArgs, ToolChoiceOptions,
        },
        responses::{
            CreateResponse, EasyInputContent, EasyInputMessage, FunctionCallOutput,
            FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, InputItem, InputParam,
            Item, MessageItem, MessageType, OutputItem, OutputMessage, OutputMessageContent,
            OutputStatus, ReasoningItem, Role, SummaryPart, SummaryTextContent, Tool,
            ToolChoiceOptions as ResponsesToolChoiceOptions, ToolChoiceParam,
        },
    },
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::{
    Client as HttpClient,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::{Value, json};
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
    pub items: Vec<ChatMessage>,
}

#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Generate one model response from structured messages and tool definitions.
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse>;
}

/// Build the configured LLM provider implementation.
pub fn build_provider(config: &LLMConfig) -> Result<Arc<dyn LLMProvider>> {
    let resolved_config = config
        .resolve_active_provider()
        .context("failed to resolve active llm provider")?;
    match LLMProviderProtocol::from_config(&resolved_config)? {
        LLMProviderProtocol::Mock => Ok(Arc::new(MockLLMProvider::new(
            resolved_config.mock_response.clone(),
        ))),
        LLMProviderProtocol::OpenAiCompatible => Ok(Arc::new(OpenaiChatCompletionsProvider::new(
            resolve_provider_config(&resolved_config)?,
        )?)),
        LLMProviderProtocol::OpenAiResponses => Ok(Arc::new(ResponsesProvider::new(
            resolve_provider_config(&resolved_config)?,
        )?)),
        LLMProviderProtocol::Anthropic => Ok(Arc::new(AnthropicProvider::new(
            resolve_provider_config(&resolved_config)?,
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
    OpenAiResponses,
    Anthropic,
}

impl LLMProviderProtocol {
    fn from_config(config: &ResolvedLLMProviderConfig) -> Result<Self> {
        match config.effective_protocol() {
            "mock" => Ok(Self::Mock),
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            "openai_responses" => Ok(Self::OpenAiResponses),
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
        Ok(LLMResponse {
            items: vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                self.response.clone(),
                Utc::now(),
            )],
        })
    }
}

pub struct OpenaiChatCompletionsProvider {
    config: ResolvedLLMProviderConfig,
    client: Client<OpenAIConfig>,
}

impl OpenaiChatCompletionsProvider {
    fn new(config: ResolvedLLMProviderConfig) -> Result<Self> {
        validate_api_backed_provider_fields(&config, "openai_compatible")?;

        let mut client_config = OpenAIConfig::new()
            .with_api_key(config.api_key.clone())
            .with_api_base(config.base_url.clone());
        for (header_name, header_value) in config.headers.clone() {
            let parsed_header_name =
                HeaderName::from_bytes(header_name.as_bytes()).with_context(|| {
                    format!(
                        "failed to parse header `{header_name}` for llm provider `{}`",
                        config.name
                    )
                })?;
            client_config = client_config
                .with_header(parsed_header_name, header_value.as_str())
                .with_context(|| {
                    format!(
                        "failed to configure header `{header_name}` for llm provider `{}`",
                        config.name
                    )
                })?;
        }

        Ok(Self {
            config,
            client: Client::with_config(client_config),
        })
    }
}

pub struct ResponsesProvider {
    config: ResolvedLLMProviderConfig,
    client: HttpClient,
}

impl ResponsesProvider {
    fn new(config: ResolvedLLMProviderConfig) -> Result<Self> {
        validate_api_backed_provider_fields(&config, "openai_responses")?;
        Ok(Self {
            config,
            client: HttpClient::new(),
        })
    }
}

pub struct AnthropicProvider {
    config: ResolvedLLMProviderConfig,
}

impl AnthropicProvider {
    fn new(config: ResolvedLLMProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl LLMProvider for OpenaiChatCompletionsProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        if request.messages.is_empty() {
            bail!("llm request must contain at least one message");
        }

        let message_count = request.messages.len();
        let tool_count = request.tools.len();
        let started_at = Instant::now();
        debug!(
            protocol = self.config.effective_protocol(),
            provider = %self.config.name,
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
                    provider = %self.config.name,
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
                        self.config.name, self.config.model, self.config.base_url
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
            provider = %self.config.name,
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
        let mut items = Vec::new();
        let created_at = Utc::now();
        if let Some(content) = choice.message.content
            && !content.trim().is_empty()
        {
            items.push(ChatMessage::new(
                ChatMessageRole::Assistant,
                content,
                created_at,
            ));
        }
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(parse_openai_tool_call)
            .collect::<Result<Vec<_>>>()?;
        items.extend(tool_calls.into_iter().map(|tool_call| {
            ChatMessage::new(ChatMessageRole::Toolcall, "", created_at)
                .with_tool_calls(vec![tool_call])
        }));

        Ok(LLMResponse { items })
    }
}

#[async_trait]
impl LLMProvider for ResponsesProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        if request.messages.is_empty() {
            bail!("llm request must contain at least one message");
        }

        let message_count = request.messages.len();
        let tool_count = request.tools.len();
        let system_message_count = request
            .messages
            .iter()
            .filter(|message| message.role == ChatMessageRole::System)
            .count();
        let continuation_item_count = request
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    message.role,
                    ChatMessageRole::Assistant
                        | ChatMessageRole::Reasoning
                        | ChatMessageRole::Toolcall
                        | ChatMessageRole::ToolResult
                )
            })
            .count();
        let responses_request = build_responses_request(&self.config, &request)?;
        let input_item_count = match &responses_request.input {
            InputParam::Text(text) => usize::from(!text.trim().is_empty()),
            InputParam::Items(items) => items.len(),
        };
        let started_at = Instant::now();
        debug!(
            protocol = self.config.effective_protocol(),
            provider = %self.config.name,
            model = %self.config.model,
            base_url = %self.config.base_url,
            message_count,
            tool_count,
            system_message_count,
            input_item_count,
            continuation_item_count,
            has_instructions = responses_request.instructions.is_some(),
            "starting responses api request"
        );

        let response = match self
            .client
            .post(build_responses_url(&self.config.base_url))
            .headers(build_responses_headers(&self.config)?)
            .json(&responses_request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                debug!(
                    protocol = self.config.effective_protocol(),
                    provider = %self.config.name,
                    model = %self.config.model,
                    base_url = %self.config.base_url,
                    message_count,
                    tool_count,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "responses api network request failed"
                );
                return Err(error).with_context(|| {
                    format!(
                        "failed to call responses provider `{}` model `{}` at `{}`",
                        self.config.name, self.config.model, self.config.base_url
                    )
                });
            }
        };

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read responses api response body")?;
        if !status.is_success() {
            debug!(
                protocol = self.config.effective_protocol(),
                provider = %self.config.name,
                model = %self.config.model,
                base_url = %self.config.base_url,
                message_count,
                tool_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                status = %status,
                body = %body,
                "responses api request returned non-success status"
            );
            bail!(
                "responses api request failed with status {} for provider `{}`",
                status,
                self.config.name
            );
        }

        let api_response: async_openai::types::responses::Response =
            serde_json::from_str(&body).context("failed to parse responses api response body")?;
        if let Some(error) = &api_response.error {
            bail!("responses api returned `{}`: {}", error.code, error.message);
        }

        let items = normalize_responses_output(&api_response)?;
        debug!(
            protocol = self.config.effective_protocol(),
            provider = %self.config.name,
            model = %self.config.model,
            base_url = %self.config.base_url,
            message_count,
            tool_count,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            output_item_count = api_response.output.len(),
            normalized_item_count = items.len(),
            output_text = api_response.output_text().unwrap_or_default(),
            "completed responses api request"
        );

        Ok(LLMResponse { items })
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        let _ = build_anthropic_request(&self.config, &request)?;
        bail!("provider protocol `anthropic` is not implemented yet")
    }
}

fn build_openai_request(
    config: &ResolvedLLMProviderConfig,
    request: LLMRequest,
) -> Result<CreateChatCompletionRequest> {
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
    let mut serialized = Vec::with_capacity(messages.len());
    let mut cursor = 0usize;
    while cursor < messages.len() {
        let message = &messages[cursor];
        match message.role {
            ChatMessageRole::Reasoning => {
                cursor += 1;
            }
            ChatMessageRole::Toolcall => {
                let (tool_call_message, consumed) =
                    collect_tool_call_messages(messages, cursor, None);
                serialized.push(serialize_assistant_message(&tool_call_message)?.into());
                cursor += consumed;
            }
            ChatMessageRole::Assistant if message_starts_tool_call_batch(messages, cursor) => {
                let next_index = next_non_reasoning_index(messages, cursor + 1)
                    .expect("assistant tool-call batch should have next item");
                let (assistant_message, consumed) =
                    collect_tool_call_messages(messages, next_index, Some(message));
                serialized.push(serialize_assistant_message(&assistant_message)?.into());
                cursor = next_index + consumed;
            }
            _ => {
                serialized.push(serialize_openai_context_message(message)?);
                cursor += 1;
            }
        }
    }

    Ok(serialized)
}

fn serialize_openai_context_message(message: &ChatMessage) -> Result<ChatCompletionRequestMessage> {
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
        ChatMessageRole::Reasoning => {
            bail!("reasoning messages must be filtered before chat-completion serialization")
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

fn next_non_reasoning_index(messages: &[ChatMessage], start: usize) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, message)| (message.role != ChatMessageRole::Reasoning).then_some(index))
}

fn message_starts_tool_call_batch(messages: &[ChatMessage], cursor: usize) -> bool {
    let message = &messages[cursor];
    if !message.tool_calls.is_empty() {
        return true;
    }

    next_non_reasoning_index(messages, cursor + 1)
        .and_then(|index| messages.get(index))
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
        .unwrap_or_else(Utc::now);
    let content = assistant_message
        .map(|message| message.content.clone())
        .unwrap_or_default();
    let mut consumed = 0usize;

    while let Some(message) = messages.get(start + consumed) {
        if message.role == ChatMessageRole::Reasoning {
            consumed += 1;
            continue;
        }
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

fn build_responses_request(
    config: &ResolvedLLMProviderConfig,
    request: &LLMRequest,
) -> Result<CreateResponse> {
    let input = serialize_responses_input_items(&request.messages)?;
    let tools = serialize_responses_tools(&request.tools)?;

    Ok(CreateResponse {
        model: Some(config.model.clone()),
        input: InputParam::Items(input),
        instructions: join_system_instructions(&request.messages),
        max_output_tokens: Some(u32::try_from(config.max_output_tokens()).unwrap_or(u32::MAX)),
        parallel_tool_calls: Some(false),
        store: Some(false),
        temperature: Some(0.1),
        tool_choice: (!tools.is_empty())
            .then_some(ToolChoiceParam::Mode(ResponsesToolChoiceOptions::Auto)),
        tools: (!tools.is_empty()).then_some(tools),
        ..CreateResponse::default()
    })
}

fn serialize_responses_input_items(messages: &[ChatMessage]) -> Result<Vec<InputItem>> {
    let mut items = Vec::new();
    for message in messages {
        if message.role == ChatMessageRole::System {
            continue;
        }
        match message.role {
            ChatMessageRole::User => {
                items.push(build_easy_input_message(Role::User, &message.content))
            }
            ChatMessageRole::Assistant => items.push(build_responses_assistant_input(message)),
            ChatMessageRole::Reasoning => items.push(build_responses_reasoning_input(message)?),
            ChatMessageRole::Toolcall => items.push(build_responses_function_call_input(message)?),
            ChatMessageRole::ToolResult => {
                items.push(build_responses_function_call_output_input(message)?)
            }
            ChatMessageRole::System => {}
        }
    }

    Ok(items)
}

fn build_easy_input_message(role: Role, content: &str) -> InputItem {
    InputItem::EasyMessage(EasyInputMessage {
        r#type: MessageType::Message,
        role,
        content: EasyInputContent::Text(content.to_string()),
    })
}

fn build_responses_assistant_input(message: &ChatMessage) -> InputItem {
    if let Some(provider_item_id) = message.provider_item_id.clone() {
        let mut content = Vec::new();
        if !message.content.trim().is_empty() {
            content.push(OutputMessageContent::OutputText(
                async_openai::types::responses::OutputTextContent {
                    annotations: Vec::new(),
                    logprobs: None,
                    text: message.content.clone(),
                },
            ));
        }
        return InputItem::Item(Item::Message(MessageItem::Output(OutputMessage {
            content,
            id: provider_item_id,
            role: async_openai::types::responses::AssistantRole::Assistant,
            status: OutputStatus::Completed,
        })));
    }

    build_easy_input_message(Role::Assistant, &message.content)
}

fn build_responses_reasoning_input(message: &ChatMessage) -> Result<InputItem> {
    let provider_item_id = message
        .provider_item_id
        .clone()
        .context("reasoning message is missing provider_item_id for responses continuation")?;
    Ok(InputItem::Item(Item::Reasoning(ReasoningItem {
        id: provider_item_id,
        summary: vec![SummaryPart::SummaryText(SummaryTextContent {
            text: message.content.clone(),
        })],
        content: None,
        encrypted_content: None,
        status: Some(OutputStatus::Completed),
    })))
}

fn build_responses_function_call_input(message: &ChatMessage) -> Result<InputItem> {
    let tool_call = first_tool_call(message)?;
    Ok(InputItem::Item(Item::FunctionCall(FunctionToolCall {
        arguments: tool_call.arguments.to_string(),
        call_id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        id: message
            .provider_item_id
            .clone()
            .or_else(|| tool_call.provider_item_id.clone()),
        status: Some(OutputStatus::Completed),
    })))
}

fn build_responses_function_call_output_input(message: &ChatMessage) -> Result<InputItem> {
    Ok(InputItem::Item(Item::FunctionCallOutput(
        FunctionCallOutputItemParam {
            call_id: message.tool_call_id.clone().context(
                "tool result message is missing tool_call_id for responses continuation",
            )?,
            output: FunctionCallOutput::Text(message.content.clone()),
            id: message.provider_item_id.clone(),
            status: Some(OutputStatus::Completed),
        },
    )))
}

fn serialize_responses_tools(tools: &[ToolDefinition]) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function(FunctionTool {
                name: tool.name.clone(),
                parameters: Some(tool.input_schema.for_protocol(ToolSchemaProtocol::OpenAi)),
                strict: Some(false),
                description: Some(tool.description.clone()),
            }))
        })
        .collect()
}

fn normalize_responses_output(
    response: &async_openai::types::responses::Response,
) -> Result<Vec<ChatMessage>> {
    let created_at = response_output_created_at(response);
    let mut items = Vec::new();
    for output_item in &response.output {
        match output_item {
            OutputItem::Reasoning(reasoning) => items.push(
                ChatMessage::new(
                    ChatMessageRole::Reasoning,
                    build_reasoning_text(reasoning),
                    created_at,
                )
                .with_provider_item_id(reasoning.id.clone()),
            ),
            OutputItem::FunctionCall(function_call) => {
                let arguments = serde_json::from_str::<Value>(&function_call.arguments)
                    .with_context(|| {
                        format!(
                            "failed to parse responses tool arguments for `{}`",
                            function_call.name
                        )
                    })?;
                let tool_call = ChatToolCall {
                    id: function_call.call_id.clone(),
                    name: function_call.name.clone(),
                    arguments,
                    provider_item_id: function_call.id.clone(),
                };
                let mut message = ChatMessage::new(ChatMessageRole::Toolcall, "", created_at)
                    .with_tool_calls(vec![tool_call]);
                if let Some(provider_item_id) = &function_call.id {
                    message = message.with_provider_item_id(provider_item_id.clone());
                }
                items.push(message);
            }
            OutputItem::Message(message) => {
                let mut normalized = ChatMessage::new(
                    ChatMessageRole::Assistant,
                    build_output_message_text(&message.content),
                    created_at,
                );
                normalized = normalized.with_provider_item_id(message.id.clone());
                items.push(normalized);
            }
            unsupported => {
                bail!("responses output item `{unsupported:?}` is not supported yet");
            }
        }
    }

    Ok(items)
}

fn response_output_created_at(
    response: &async_openai::types::responses::Response,
) -> chrono::DateTime<Utc> {
    let timestamp = response.completed_at.unwrap_or(response.created_at) as i64;
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

fn build_output_message_text(content: &[OutputMessageContent]) -> String {
    content
        .iter()
        .map(|part| match part {
            OutputMessageContent::OutputText(text) => text.text.clone(),
            OutputMessageContent::Refusal(refusal) => refusal.refusal.clone(),
        })
        .collect::<Vec<_>>()
        .join("")
}

fn build_reasoning_text(reasoning: &ReasoningItem) -> String {
    let summary = reasoning
        .summary
        .iter()
        .map(|part| match part {
            SummaryPart::SummaryText(text) => text.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !summary.trim().is_empty() {
        return summary;
    }

    reasoning
        .content
        .as_ref()
        .map(|parts| {
            parts
                .iter()
                .map(|part| part.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn build_anthropic_request(
    config: &ResolvedLLMProviderConfig,
    request: &LLMRequest,
) -> Result<Value> {
    if request.messages.is_empty() {
        bail!("llm request must contain at least one message");
    }

    Ok(json!({
        "model": config.model,
        "system": join_system_instructions(&request.messages),
        "messages": serialize_anthropic_messages(&request.messages)?,
        "tools": serialize_anthropic_tools(&request.tools)?,
        "max_tokens": config.max_output_tokens(),
    }))
}

fn serialize_anthropic_messages(messages: &[ChatMessage]) -> Result<Vec<Value>> {
    let mut serialized = Vec::new();
    for message in messages {
        match message.role {
            ChatMessageRole::System | ChatMessageRole::Reasoning => {}
            ChatMessageRole::User => serialized.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": message.content.clone() }],
            })),
            ChatMessageRole::Assistant => serialized.push(json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": message.content.clone() }],
            })),
            ChatMessageRole::Toolcall => {
                let tool_call = first_tool_call(message)?;
                serialized.push(json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": tool_call.id,
                        "name": tool_call.name,
                        "input": tool_call.arguments,
                    }],
                }));
            }
            ChatMessageRole::ToolResult => serialized.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().context(
                        "tool result message is missing tool_call_id for anthropic projection"
                    )?,
                    "content": message.content.clone(),
                }],
            })),
        }
    }

    Ok(serialized)
}

fn serialize_anthropic_tools(tools: &[ToolDefinition]) -> Result<Vec<Value>> {
    Ok(tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name.clone(),
                "description": tool.description.clone(),
                "input_schema": tool.input_schema.for_protocol(ToolSchemaProtocol::Anthropic),
            })
        })
        .collect())
}

fn join_system_instructions(messages: &[ChatMessage]) -> Option<String> {
    let instructions = messages
        .iter()
        .filter(|message| message.role == ChatMessageRole::System)
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if instructions.is_empty() {
        None
    } else {
        Some(instructions)
    }
}

fn first_tool_call(message: &ChatMessage) -> Result<&ChatToolCall> {
    let tool_call_count = message.tool_calls.len();
    if tool_call_count != 1 {
        bail!("toolcall message must contain exactly one tool call item, got {tool_call_count}");
    }
    Ok(&message.tool_calls[0])
}

fn serialize_openai_tool_call(tool_call: &LLMToolCall) -> ChatCompletionMessageToolCalls {
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
                provider_item_id: None,
            })
        }
        ChatCompletionMessageToolCalls::Custom(tool_call) => bail!(
            "custom tool calls are not supported yet: `{}`",
            tool_call.custom_tool.name
        ),
    }
}

fn validate_api_backed_provider_fields(
    config: &ResolvedLLMProviderConfig,
    protocol: &str,
) -> Result<()> {
    if config.api_key.trim().is_empty() {
        bail!(
            "llm active provider `{}` is missing api_key when protocol={protocol}",
            config.name
        );
    }
    if config.model.trim().is_empty() {
        bail!(
            "llm active provider `{}` is missing model when protocol={protocol}",
            config.name
        );
    }

    Ok(())
}

fn resolve_provider_config(
    config: &ResolvedLLMProviderConfig,
) -> Result<ResolvedLLMProviderConfig> {
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

fn build_responses_headers(config: &ResolvedLLMProviderConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.api_key))
            .context("failed to build Authorization header for responses provider")?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    for (header_name, header_value) in &config.headers {
        headers.insert(
            HeaderName::from_bytes(header_name.as_bytes())
                .with_context(|| format!("invalid responses header name `{header_name}`"))?,
            HeaderValue::from_str(header_value)
                .with_context(|| format!("invalid responses header value for `{header_name}`"))?,
        );
    }

    Ok(headers)
}

fn build_responses_url(base_url: &str) -> String {
    let trimmed_base_url = base_url.trim_end_matches('/');
    if trimmed_base_url.ends_with("/responses") {
        trimmed_base_url.to_string()
    } else {
        format!("{trimmed_base_url}/responses")
    }
}

fn expand_home_dir(path: &Path) -> Result<PathBuf> {
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
    if let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
    {
        return Ok(PathBuf::from(home));
    }
    if let Ok(user_profile) = env::var("USERPROFILE")
        && !user_profile.trim().is_empty()
    {
        return Ok(PathBuf::from(user_profile));
    }

    bail!("failed to resolve user home directory for api_key_path")
}

#[cfg(test)]
mod tests {
    use super::{
        build_responses_request, build_responses_url, normalize_responses_output,
        serialize_anthropic_messages, serialize_openai_messages,
    };
    use crate::{
        agent::{ToolDefinition, ToolSource, empty_tool_input_schema},
        config::ResolvedLLMProviderConfig,
        context::{ChatMessage, ChatMessageRole, ChatToolCall},
    };
    use async_openai::types::responses::{
        FunctionToolCall, InputItem, InputParam, Item, OutputItem, OutputMessage,
        OutputMessageContent, OutputStatus, ReasoningItem, Status, SummaryPart, SummaryTextContent,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn serialize_openai_messages_merges_split_toolcall_messages() {
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
                    provider_item_id: None,
                },
            ]),
            ChatMessage::new(ChatMessageRole::Toolcall, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_2".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "src/main.rs" }),
                    provider_item_id: None,
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

    #[test]
    fn build_responses_request_keeps_prior_output_items_and_tool_outputs() {
        let now = Utc::now();
        let request = build_responses_request(
            &ResolvedLLMProviderConfig {
                name: "responses".to_string(),
                protocol: "openai_responses".to_string(),
                model: "gpt-5-mini".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "test-key".to_string(),
                api_key_path: Default::default(),
                mock_response: String::new(),
                context_window_tokens: None,
                max_output_tokens: Some(1024),
                tokenizer: "chars_div4".to_string(),
                headers: HashMap::new(),
            },
            &crate::llm::LLMRequest {
                messages: vec![
                    ChatMessage::new(ChatMessageRole::System, "system", now),
                    ChatMessage::new(ChatMessageRole::User, "北京天气怎么样？", now),
                    ChatMessage::new(ChatMessageRole::Reasoning, "需要调用天气工具", now)
                        .with_provider_item_id("rsn_1"),
                    ChatMessage::new(ChatMessageRole::Toolcall, "", now)
                        .with_provider_item_id("fc_1")
                        .with_tool_calls(vec![ChatToolCall {
                            id: "call_1".to_string(),
                            name: "get_weather".to_string(),
                            arguments: json!({ "city": "北京" }),
                            provider_item_id: Some("fc_1".to_string()),
                        }]),
                    ChatMessage::new(ChatMessageRole::ToolResult, "{\"temp\":15}", now)
                        .with_tool_call_id("call_1"),
                ],
                tools: vec![ToolDefinition {
                    name: "get_weather".to_string(),
                    description: "read weather".to_string(),
                    input_schema: empty_tool_input_schema(),
                    source: ToolSource::Builtin,
                }],
            },
        )
        .expect("responses request should build");

        match request.input {
            InputParam::Items(items) => {
                assert_eq!(items.len(), 4);
                assert!(matches!(items[1], InputItem::Item(Item::Reasoning(_))));
                assert!(matches!(items[2], InputItem::Item(Item::FunctionCall(_))));
                assert!(matches!(
                    items[3],
                    InputItem::Item(Item::FunctionCallOutput(_))
                ));
            }
            InputParam::Text(_) => panic!("responses request should use item input"),
        }
        assert_eq!(request.instructions.as_deref(), Some("system"));
    }

    #[test]
    fn build_responses_url_keeps_explicit_responses_suffix() {
        assert_eq!(
            build_responses_url("https://api.openai.com/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            build_responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn build_responses_request_replays_multi_tool_itinerary_conversation() {
        // 测试场景: Responses continuation 需要无损回放推理、多个 function_call 和对应的 function_call_output。
        let now = Utc::now();
        let request = build_responses_request(
            &ResolvedLLMProviderConfig {
                name: "responses".to_string(),
                protocol: "openai_responses".to_string(),
                model: "gpt-5-mini".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "test-key".to_string(),
                api_key_path: Default::default(),
                mock_response: String::new(),
                context_window_tokens: None,
                max_output_tokens: Some(1024),
                tokenizer: "chars_div4".to_string(),
                headers: HashMap::new(),
            },
            &crate::llm::LLMRequest {
                messages: vec![
                    ChatMessage::new(ChatMessageRole::System, "system", now),
                    ChatMessage::new(ChatMessageRole::User, "帮我规划杭州三日行程", now),
                    ChatMessage::new(ChatMessageRole::Reasoning, "先查杭州天气", now)
                        .with_provider_item_id("rsn_weather"),
                    ChatMessage::new(ChatMessageRole::Toolcall, "", now)
                        .with_provider_item_id("fc_weather")
                        .with_tool_calls(vec![ChatToolCall {
                            id: "call_weather".to_string(),
                            name: "get_weather".to_string(),
                            arguments: json!({ "city": "杭州" }),
                            provider_item_id: Some("fc_weather".to_string()),
                        }]),
                    ChatMessage::new(ChatMessageRole::ToolResult, "{\"forecast\":\"sunny\"}", now)
                        .with_tool_call_id("call_weather"),
                    ChatMessage::new(ChatMessageRole::Reasoning, "天气合适，再查高铁和酒店", now)
                        .with_provider_item_id("rsn_trip"),
                    ChatMessage::new(ChatMessageRole::Toolcall, "", now)
                        .with_provider_item_id("fc_train")
                        .with_tool_calls(vec![ChatToolCall {
                            id: "call_train".to_string(),
                            name: "search_train".to_string(),
                            arguments: json!({ "from": "上海", "to": "杭州" }),
                            provider_item_id: Some("fc_train".to_string()),
                        }]),
                    ChatMessage::new(ChatMessageRole::Toolcall, "", now)
                        .with_provider_item_id("fc_hotel")
                        .with_tool_calls(vec![ChatToolCall {
                            id: "call_hotel".to_string(),
                            name: "search_hotel".to_string(),
                            arguments: json!({ "city": "杭州", "nights": 2 }),
                            provider_item_id: Some("fc_hotel".to_string()),
                        }]),
                    ChatMessage::new(
                        ChatMessageRole::ToolResult,
                        "{\"departures\":[\"G7311\"]}",
                        now,
                    )
                    .with_tool_call_id("call_train"),
                    ChatMessage::new(
                        ChatMessageRole::ToolResult,
                        "{\"hotels\":[\"West Lake Hotel\"]}",
                        now,
                    )
                    .with_tool_call_id("call_hotel"),
                ],
                tools: vec![
                    ToolDefinition {
                        name: "get_weather".to_string(),
                        description: "read weather".to_string(),
                        input_schema: empty_tool_input_schema(),
                        source: ToolSource::Builtin,
                    },
                    ToolDefinition {
                        name: "search_train".to_string(),
                        description: "search train".to_string(),
                        input_schema: empty_tool_input_schema(),
                        source: ToolSource::Builtin,
                    },
                    ToolDefinition {
                        name: "search_hotel".to_string(),
                        description: "search hotel".to_string(),
                        input_schema: empty_tool_input_schema(),
                        source: ToolSource::Builtin,
                    },
                ],
            },
        )
        .expect("responses multi-tool request should build");

        match request.input {
            InputParam::Items(items) => {
                assert_eq!(items.len(), 9);
                assert!(matches!(items[1], InputItem::Item(Item::Reasoning(_))));
                assert!(matches!(items[2], InputItem::Item(Item::FunctionCall(_))));
                assert!(matches!(
                    items[3],
                    InputItem::Item(Item::FunctionCallOutput(_))
                ));
                assert!(matches!(items[4], InputItem::Item(Item::Reasoning(_))));
                assert!(matches!(items[5], InputItem::Item(Item::FunctionCall(_))));
                assert!(matches!(items[6], InputItem::Item(Item::FunctionCall(_))));
                assert!(matches!(
                    items[7],
                    InputItem::Item(Item::FunctionCallOutput(_))
                ));
                assert!(matches!(
                    items[8],
                    InputItem::Item(Item::FunctionCallOutput(_))
                ));
            }
            InputParam::Text(_) => panic!("responses request should use item input"),
        }
    }

    #[test]
    fn build_responses_request_rejects_reasoning_without_provider_item_id() {
        // 测试场景: reasoning continuation 必须回放 provider item id，否则下一轮 Responses 请求会丢失上游 item 身份。
        let now = Utc::now();
        let error = build_responses_request(
            &ResolvedLLMProviderConfig {
                name: "responses".to_string(),
                protocol: "openai_responses".to_string(),
                model: "gpt-5-mini".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: "test-key".to_string(),
                api_key_path: Default::default(),
                mock_response: String::new(),
                context_window_tokens: None,
                max_output_tokens: Some(1024),
                tokenizer: "chars_div4".to_string(),
                headers: HashMap::new(),
            },
            &crate::llm::LLMRequest {
                messages: vec![
                    ChatMessage::new(ChatMessageRole::System, "system", now),
                    ChatMessage::new(ChatMessageRole::User, "北京天气怎么样？", now),
                    ChatMessage::new(ChatMessageRole::Reasoning, "需要调用天气工具", now),
                ],
                tools: vec![ToolDefinition {
                    name: "get_weather".to_string(),
                    description: "read weather".to_string(),
                    input_schema: empty_tool_input_schema(),
                    source: ToolSource::Builtin,
                }],
            },
        )
        .expect_err("responses continuation should reject reasoning without provider item id");

        assert!(
            error
                .to_string()
                .contains("reasoning message is missing provider_item_id")
        );
    }

    #[test]
    fn normalize_responses_output_preserves_reasoning_toolcall_and_assistant_order() {
        let response = async_openai::types::responses::Response {
            background: None,
            billing: None,
            conversation: None,
            created_at: 1_744_330_000,
            completed_at: Some(1_744_330_001),
            error: None,
            id: "resp_1".to_string(),
            incomplete_details: None,
            instructions: None,
            max_output_tokens: None,
            metadata: None,
            model: "gpt-5-mini".to_string(),
            object: "response".to_string(),
            output: vec![
                OutputItem::Reasoning(ReasoningItem {
                    id: "rsn_1".to_string(),
                    summary: vec![SummaryPart::SummaryText(SummaryTextContent {
                        text: "先查天气".to_string(),
                    })],
                    content: None,
                    encrypted_content: None,
                    status: Some(OutputStatus::Completed),
                }),
                OutputItem::FunctionCall(FunctionToolCall {
                    arguments: "{\"city\":\"北京\"}".to_string(),
                    call_id: "call_1".to_string(),
                    name: "get_weather".to_string(),
                    id: Some("fc_1".to_string()),
                    status: Some(OutputStatus::Completed),
                }),
                OutputItem::Message(OutputMessage {
                    content: vec![OutputMessageContent::OutputText(
                        async_openai::types::responses::OutputTextContent {
                            annotations: Vec::new(),
                            logprobs: None,
                            text: "北京今天多云".to_string(),
                        },
                    )],
                    id: "msg_1".to_string(),
                    role: async_openai::types::responses::AssistantRole::Assistant,
                    status: OutputStatus::Completed,
                }),
            ],
            parallel_tool_calls: Some(false),
            previous_response_id: None,
            prompt_cache_key: None,
            reasoning: None,
            safety_identifier: None,
            service_tier: None,
            status: Status::Completed,
            temperature: None,
            text: None,
            tool_choice: None,
            tools: None,
            top_logprobs: None,
            top_p: None,
            truncation: None,
            usage: None,
            prompt_cache_retention: None,
            prompt: None,
        };

        let items = normalize_responses_output(&response).expect("response should normalize");
        assert_eq!(
            items
                .iter()
                .map(|item| item.role.clone())
                .collect::<Vec<_>>(),
            vec![
                ChatMessageRole::Reasoning,
                ChatMessageRole::Toolcall,
                ChatMessageRole::Assistant,
            ]
        );
        assert_eq!(items[0].provider_item_id.as_deref(), Some("rsn_1"));
        assert_eq!(items[1].tool_calls[0].id, "call_1");
        assert_eq!(
            items[1].tool_calls[0].provider_item_id.as_deref(),
            Some("fc_1")
        );
        assert_eq!(items[2].content, "北京今天多云");
    }

    #[test]
    fn normalize_responses_output_preserves_multi_tool_itinerary_order() {
        // 测试场景: itinerary 这类多工具响应必须按原顺序保留多个 function_call，避免上层 ReAct tool loop 错位。
        let response = async_openai::types::responses::Response {
            background: None,
            billing: None,
            conversation: None,
            created_at: 1_744_330_100,
            completed_at: Some(1_744_330_101),
            error: None,
            id: "resp_trip".to_string(),
            incomplete_details: None,
            instructions: None,
            max_output_tokens: None,
            metadata: None,
            model: "gpt-5-mini".to_string(),
            object: "response".to_string(),
            output: vec![
                OutputItem::Reasoning(ReasoningItem {
                    id: "rsn_trip".to_string(),
                    summary: vec![SummaryPart::SummaryText(SummaryTextContent {
                        text: "先查高铁和酒店".to_string(),
                    })],
                    content: None,
                    encrypted_content: None,
                    status: Some(OutputStatus::Completed),
                }),
                OutputItem::FunctionCall(FunctionToolCall {
                    arguments: "{\"from\":\"上海\",\"to\":\"杭州\"}".to_string(),
                    call_id: "call_train".to_string(),
                    name: "search_train".to_string(),
                    id: Some("fc_train".to_string()),
                    status: Some(OutputStatus::Completed),
                }),
                OutputItem::FunctionCall(FunctionToolCall {
                    arguments: "{\"city\":\"杭州\",\"nights\":2}".to_string(),
                    call_id: "call_hotel".to_string(),
                    name: "search_hotel".to_string(),
                    id: Some("fc_hotel".to_string()),
                    status: Some(OutputStatus::Completed),
                }),
                OutputItem::Message(OutputMessage {
                    content: vec![OutputMessageContent::OutputText(
                        async_openai::types::responses::OutputTextContent {
                            annotations: Vec::new(),
                            logprobs: None,
                            text: "已为你整理杭州三日行程".to_string(),
                        },
                    )],
                    id: "msg_trip".to_string(),
                    role: async_openai::types::responses::AssistantRole::Assistant,
                    status: OutputStatus::Completed,
                }),
            ],
            parallel_tool_calls: Some(true),
            previous_response_id: None,
            prompt_cache_key: None,
            reasoning: None,
            safety_identifier: None,
            service_tier: None,
            status: Status::Completed,
            temperature: None,
            text: None,
            tool_choice: None,
            tools: None,
            top_logprobs: None,
            top_p: None,
            truncation: None,
            usage: None,
            prompt_cache_retention: None,
            prompt: None,
        };

        let items = normalize_responses_output(&response).expect("response should normalize");
        assert_eq!(
            items
                .iter()
                .map(|item| item.role.clone())
                .collect::<Vec<_>>(),
            vec![
                ChatMessageRole::Reasoning,
                ChatMessageRole::Toolcall,
                ChatMessageRole::Toolcall,
                ChatMessageRole::Assistant,
            ]
        );
        assert_eq!(items[1].tool_calls[0].id, "call_train");
        assert_eq!(items[2].tool_calls[0].id, "call_hotel");
        assert_eq!(
            items[1].tool_calls[0].provider_item_id.as_deref(),
            Some("fc_train")
        );
        assert_eq!(
            items[2].tool_calls[0].provider_item_id.as_deref(),
            Some("fc_hotel")
        );
        assert_eq!(items[3].content, "已为你整理杭州三日行程");
    }

    #[test]
    fn anthropic_projection_uses_shared_chat_message_model() {
        let now = Utc::now();
        let payload = serialize_anthropic_messages(&[
            ChatMessage::new(ChatMessageRole::User, "查天气", now),
            ChatMessage::new(ChatMessageRole::Reasoning, "先调用工具", now)
                .with_provider_item_id("rsn_1"),
            ChatMessage::new(ChatMessageRole::Toolcall, "", now).with_tool_calls(vec![
                ChatToolCall {
                    id: "call_1".to_string(),
                    name: "get_weather".to_string(),
                    arguments: json!({ "city": "北京" }),
                    provider_item_id: Some("fc_1".to_string()),
                },
            ]),
            ChatMessage::new(ChatMessageRole::ToolResult, "{\"temp\":15}", now)
                .with_tool_call_id("call_1"),
        ])
        .expect("anthropic projection should succeed");

        assert_eq!(payload.len(), 3);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[1]["content"][0]["type"], "tool_use");
        assert_eq!(payload[2]["content"][0]["type"], "tool_result");
    }
}
