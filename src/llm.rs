use crate::{
    agent::ToolDefinition,
    config::LlmConfig,
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
    /// 作用: 根据当前请求中的结构化消息和工具定义生成模型输出。
    /// 参数: request 为已经整理好的 ChatMessage、tools 和工具执行上下文。
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse>;
}

pub fn build_provider(config: &LlmConfig) -> Result<Arc<dyn LLMProvider>> {
    // 作用: 按配置构造具体的 LLM provider 实现。
    // 参数: config 为 llm 子配置，决定 provider 类型和所需鉴权信息。
    match LLMProviderProtocol::from_config(config)? {
        LLMProviderProtocol::Mock => {
            Ok(Arc::new(MockLLMProvider::new(config.mock_response.clone())))
        }
        LLMProviderProtocol::OpenAiCompatible => {
            let resolved_config = resolve_llm_config(config)?;
            Ok(Arc::new(OpenaiProvider::new(resolved_config)?))
        }
        LLMProviderProtocol::Anthropic => {
            bail!("provider protocol `anthropic` is not implemented yet")
        }
    }
}

enum LLMProviderProtocol {
    Mock,
    OpenAiCompatible,
    Anthropic,
}

impl LLMProviderProtocol {
    fn from_config(config: &LlmConfig) -> Result<Self> {
        // 作用: 从配置中的 provider 字段解析底层协议类型，统一处理别名。
        // 参数: config 为 llm 子配置，provider 字段可为 mock、deepseek、openai 等别名。
        match config.provider.trim().to_ascii_lowercase().as_str() {
            "mock" | "mock_llm" => Ok(Self::Mock),
            "openai_compatible" | "openai" | "deepseek" => Ok(Self::OpenAiCompatible),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            other => bail!("unsupported llm provider `{other}`"),
        }
    }
}

pub struct MockLLMProvider {
    response: String,
}

impl MockLLMProvider {
    pub fn new(response: impl Into<String>) -> Self {
        // 作用: 创建一个固定回包的 mock provider，用于本地链路调试。
        // 参数: response 为 generate 时直接返回的固定文本。
        Self {
            response: response.into(),
        }
    }
}

#[async_trait]
impl LLMProvider for MockLLMProvider {
    async fn generate(&self, _request: LLMRequest) -> Result<LLMResponse> {
        // 作用: 返回固定 mock 文本，不访问任何外部模型服务。
        // 参数: _request 为兼容统一接口的入参，当前不会被实际使用。
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
    config: LlmConfig,
    client: Client<OpenAIConfig>,
}

impl OpenaiProvider {
    fn new(config: LlmConfig) -> Result<Self> {
        // 作用: 创建 OpenAI 兼容协议 provider，并校验必要配置。
        // 参数: config 为 provider 的 base_url、api_key 和 model 配置。
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

#[async_trait]
impl LLMProvider for OpenaiProvider {
    async fn generate(&self, request: LLMRequest) -> Result<LLMResponse> {
        // 作用: 调用 OpenAI SDK 生成结构化回复和原生 tool calls。
        // 参数: request 为结构化 ChatMessage、tools 和工具执行上下文。
        if request.messages.is_empty() {
            bail!("llm request must contain at least one message");
        }

        let response = self
            .client
            .chat()
            .create(build_openai_request(&self.config, request)?)
            .await
            .context("failed to call llm provider")?;
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

fn build_openai_request(
    config: &LlmConfig,
    request: LLMRequest,
) -> Result<CreateChatCompletionRequest> {
    // 作用: 把统一 LLM 请求转换为 OpenAI SDK 的 chat completion 请求体。
    // 参数: config 为当前模型配置，request 为统一结构化请求。
    let messages = serialize_openai_messages(&request.messages)?;
    let tools = serialize_openai_tools(&request.tools)?;
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(config.model.clone());
    builder.messages(messages);
    builder.temperature(0.1);

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
    // 作用: 把通用 ChatMessage 列表转换成 OpenAI SDK messages 数组。
    // 参数: messages 为统一消息模型，已经包含完整的 assistant/tool 历史。
    messages
        .iter()
        .map(serialize_context_message)
        .collect::<Result<Vec<_>>>()
}

fn serialize_context_message(message: &ChatMessage) -> Result<ChatCompletionRequestMessage> {
    // 作用: 把通用 ChatMessage 转换成 OpenAI SDK 的单条 message。
    // 参数: message 为统一上下文消息。
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
        ChatMessageRole::Assistant | ChatMessageRole::Tool => {
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
    // 作用: 把工具定义转换成 OpenAI SDK 原生 `tools` 数组。
    // 参数: tools 为当前可用工具定义列表。
    tools
        .iter()
        .map(|tool| {
            let function = FunctionObjectArgs::default()
                .name(tool.name.clone())
                .description(tool.description.clone())
                .parameters(tool.parameters.clone())
                .build()
                .with_context(|| format!("failed to build tool schema for `{}`", tool.name))?;
            Ok(ChatCompletionTools::Function(ChatCompletionTool {
                function,
            }))
        })
        .collect()
}

fn serialize_openai_tool_call(tool_call: &LLMToolCall) -> ChatCompletionMessageToolCalls {
    // 作用: 把统一工具调用结构转换为 OpenAI SDK assistant message 里的 tool_calls 项。
    // 参数: tool_call 为模型已经发起的工具调用。
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
    // 作用: 把 OpenAI SDK 原生 tool_call 结构转换成统一工具调用模型。
    // 参数: tool_call 为 OpenAI assistant message 中的单条 tool_call。
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

fn resolve_llm_config(config: &LlmConfig) -> Result<LlmConfig> {
    // 作用: 解析 llm 配置中的 api key 来源，并返回可直接用于 provider 的配置副本。
    // 参数: config 为原始 llm 配置，可能包含 api_key_path。
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
    // 作用: 解析配置中的波浪线路径，展开为当前用户目录下的绝对路径。
    // 参数: path 为原始配置路径，支持 `~` 和 `~/...` 两种形式。
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
    // 作用: 获取当前进程用户目录，用于展开波浪线路径。
    // 参数: 无，优先读取 HOME，再回退 USERPROFILE。
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
