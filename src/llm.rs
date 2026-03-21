use crate::config::LlmConfig;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system_prompt: String,
    pub user_message: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// 作用: 根据当前请求生成模型输出文本。
    /// 参数: request 为已经整理好的 system prompt 和 user prompt。
    async fn generate(&self, request: LlmRequest) -> Result<String>;
}

pub fn build_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>> {
    // 作用: 按配置构造具体的 LLM provider 实现。
    // 参数: config 为 llm 子配置，决定 provider 类型和所需鉴权信息。
    match config.provider.as_str() {
        "mock" | "mock_llm" => Ok(Arc::new(MockLLMProvider::new(config.mock_response.clone()))),
        "openai_compatible" => Ok(Arc::new(OpenAiCompatibleLlmProvider::new(config.clone())?)),
        other => bail!("unsupported llm provider `{other}`"),
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
impl LlmProvider for MockLLMProvider {
    async fn generate(&self, _request: LlmRequest) -> Result<String> {
        // 作用: 返回固定 mock 文本，不访问任何外部模型服务。
        // 参数: _request 为兼容统一接口的入参，当前不会被实际使用。
        Ok(self.response.clone())
    }
}

pub struct OpenAiCompatibleLlmProvider {
    config: LlmConfig,
    client: Client,
}

impl OpenAiCompatibleLlmProvider {
    fn new(config: LlmConfig) -> Result<Self> {
        // 作用: 创建 OpenAI 兼容协议 provider，并校验必要配置。
        // 参数: config 为 provider 的 base_url、api_key 和 model 配置。
        if config.api_key.trim().is_empty() {
            bail!("llm.api_key is required when provider=openai_compatible");
        }
        if config.model.trim().is_empty() {
            bail!("llm.model is required when provider=openai_compatible");
        }

        Ok(Self {
            config,
            client: Client::new(),
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleLlmProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String> {
        // 作用: 调用 OpenAI 兼容接口生成回复文本。
        // 参数: request 为系统提示词和用户消息的组合输入。
        let endpoint = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&OpenAiChatCompletionRequest {
                model: self.config.model.clone(),
                messages: vec![
                    OpenAiChatMessage {
                        role: "system".to_string(),
                        content: request.system_prompt,
                    },
                    OpenAiChatMessage {
                        role: "user".to_string(),
                        content: request.user_message,
                    },
                ],
                temperature: 0.1,
            })
            .send()
            .await
            .context("failed to call llm provider")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read llm response")?;
        if !status.is_success() {
            bail!("llm request failed with status {status}: {body}");
        }

        let payload: OpenAiChatCompletionResponse =
            serde_json::from_str(&body).context("failed to decode llm response")?;
        let content = payload
            .choices
            .first()
            .and_then(|choice| extract_text_content(&choice.message.content))
            .filter(|content| !content.trim().is_empty())
            .context("llm response did not contain assistant text")?;
        Ok(content)
    }
}

fn extract_text_content(value: &Value) -> Option<String> {
    // 作用: 从 OpenAI 兼容响应的 content 字段中提取纯文本内容。
    // 参数: value 为 assistant message 的 content，可能是字符串或内容数组。
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let mut chunks = Vec::new();
            for item in items {
                let item_type = item.get("type").and_then(Value::as_str)?;
                if item_type != "text" {
                    continue;
                }
                let text = item.get("text").and_then(Value::as_str)?;
                chunks.push(text.to_string());
            }

            if chunks.is_empty() {
                None
            } else {
                Some(chunks.join(""))
            }
        }
        _ => None,
    }
}

#[derive(Debug, Serialize)]
struct OpenAiChatCompletionRequest {
    model: String,
    messages: Vec<OpenAiChatMessage>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct OpenAiChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiAssistantMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiAssistantMessage {
    content: Value,
}
