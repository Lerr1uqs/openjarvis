//! Compact provider abstractions and the first fixed-format prompt implementation.

use crate::{
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMProvider, LLMRequest},
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

const COMPACT_SYSTEM_PROMPT: &str = "你是 OpenJarvis 的 compact summarizer。你会把历史 chat 压缩成一条 assistant 可见的紧凑上下文。严格输出 JSON 对象，字段必须是 `compacted_assistant`。内容必须明确说明“这是压缩后的上下文”，并保留任务目标、用户约束、当前背景、当前规划、已完成、未完成和关键事实。不要输出 markdown，不要输出额外解释。";
pub const COMPACTED_ASSISTANT_PREFIX: &str = "这是压缩后的上下文，请基于这些信息继续当前任务：\n";
pub const COMPACTED_USER_CONTINUE_MESSAGE: &str = "继续";

/// Fixed-format compact request sent to a compact provider.
#[derive(Debug, Clone)]
pub struct CompactRequest {
    pub source_turn_ids: Vec<Uuid>,
    pub messages: Vec<ChatMessage>,
}

impl CompactRequest {
    /// Create one compact request from resolved source turns and messages.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::CompactRequest,
    ///     context::{ChatMessage, ChatMessageRole},
    /// };
    /// use uuid::Uuid;
    ///
    /// let request = CompactRequest::new(
    ///     vec![Uuid::new_v4()],
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())],
    /// )
    /// .expect("request should be valid");
    ///
    /// assert_eq!(request.messages.len(), 1);
    /// ```
    pub fn new(source_turn_ids: Vec<Uuid>, messages: Vec<ChatMessage>) -> Result<Self> {
        if source_turn_ids.is_empty() {
            bail!("compact request must contain at least one source turn id");
        }
        if messages.is_empty() {
            bail!("compact request must contain at least one source message");
        }

        Ok(Self {
            source_turn_ids,
            messages,
        })
    }
}

/// Structured summary returned by a compact provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactSummary {
    pub compacted_assistant: String,
}

impl CompactSummary {
    fn validate(self) -> Result<Self> {
        if self.compacted_assistant.trim().is_empty() {
            bail!("compact summary compacted_assistant must not be blank");
        }

        Ok(self)
    }
}

/// Compact provider abstraction.
#[async_trait]
pub trait CompactProvider: Send + Sync {
    async fn compact(&self, request: CompactRequest) -> Result<CompactSummary>;
}

/// Deterministic provider used by unit tests and standalone module verification.
pub struct StaticCompactProvider {
    summary: CompactSummary,
}

impl StaticCompactProvider {
    /// Create one fixed-response compact provider.
    pub fn new(summary: CompactSummary) -> Self {
        Self { summary }
    }
}

#[async_trait]
impl CompactProvider for StaticCompactProvider {
    async fn compact(&self, _request: CompactRequest) -> Result<CompactSummary> {
        Ok(self.summary.clone())
    }
}

/// Prompt fragments used by the first JSON-based compact provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactPrompt {
    pub system_prompt: String,
    pub user_prompt: String,
}

/// Build the fixed-format prompt used by the standalone compact provider.
///
/// # 示例
/// ```rust
/// use chrono::Utc;
/// use openjarvis::{
///     compact::{CompactRequest, build_compact_prompt},
///     context::{ChatMessage, ChatMessageRole},
/// };
/// use uuid::Uuid;
///
/// let request = CompactRequest::new(
///     vec![Uuid::new_v4()],
///     vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())],
/// )
/// .expect("request should build");
/// let prompt = build_compact_prompt(&request);
///
/// assert!(prompt.user_prompt.contains("[1][user] hello"));
/// ```
pub fn build_compact_prompt(request: &CompactRequest) -> CompactPrompt {
    let rendered_history = render_chat_history(&request.messages);
    CompactPrompt {
        system_prompt: COMPACT_SYSTEM_PROMPT.to_string(),
        user_prompt: format!(
            "你将收到需要被 compact 的 chat 历史。\n请输出 JSON：{{\"compacted_assistant\":\"...\"}}\nsource_turn_ids: {}\nchat_history:\n{}",
            request
                .source_turn_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            rendered_history
        ),
    }
}

/// Render one chat history into a deterministic plain-text transcript for compact prompting.
///
/// # 示例
/// ```rust
/// use chrono::Utc;
/// use openjarvis::{
///     compact::render_chat_history,
///     context::{ChatMessage, ChatMessageRole},
/// };
///
/// let rendered = render_chat_history(&[ChatMessage::new(
///     ChatMessageRole::User,
///     "hello",
///     Utc::now(),
/// )]);
///
/// assert!(rendered.contains("[1][user] hello"));
/// ```
pub fn render_chat_history(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| render_chat_message(index + 1, message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compact provider backed by the existing `LLMProvider` abstraction.
pub struct LLMCompactProvider {
    provider: Arc<dyn LLMProvider>,
}

impl LLMCompactProvider {
    /// Create one compact provider that delegates summarization to an LLM backend.
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl CompactProvider for LLMCompactProvider {
    async fn compact(&self, request: CompactRequest) -> Result<CompactSummary> {
        let prompt = build_compact_prompt(&request);
        info!(
            source_turn_count = request.source_turn_ids.len(),
            source_message_count = request.messages.len(),
            "calling compact provider"
        );

        let response = self
            .provider
            .generate(LLMRequest {
                messages: vec![
                    ChatMessage::new(
                        ChatMessageRole::System,
                        prompt.system_prompt,
                        chrono::Utc::now(),
                    ),
                    ChatMessage::new(
                        ChatMessageRole::User,
                        prompt.user_prompt,
                        chrono::Utc::now(),
                    ),
                ],
                tools: Vec::new(),
            })
            .await
            .context("compact provider failed to generate a summary")?;

        if !response.tool_calls.is_empty() {
            bail!("compact provider must not return tool calls");
        }

        let content = response
            .message
            .map(|message| message.content)
            .context("compact provider returned no assistant message")?;

        parse_compact_summary(&content)
    }
}

fn render_chat_message(index: usize, message: &ChatMessage) -> String {
    let mut line = format!("[{index}][{}] {}", message.role.as_label(), message.content);
    if !message.tool_calls.is_empty() {
        let tool_calls = message
            .tool_calls
            .iter()
            .map(|tool_call| {
                format!(
                    "{{id={},name={},arguments={}}}",
                    tool_call.id, tool_call.name, tool_call.arguments
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(&format!(" [tool_calls={tool_calls}]"));
    }
    if let Some(tool_call_id) = &message.tool_call_id {
        line.push_str(&format!(" [tool_call_id={tool_call_id}]"));
    }
    line
}

fn parse_compact_summary(raw: &str) -> Result<CompactSummary> {
    let candidate = extract_json_object(raw.trim()).unwrap_or(raw.trim());
    serde_json::from_str::<CompactSummary>(candidate)
        .context("compact provider returned invalid JSON")
        .and_then(CompactSummary::validate)
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    raw.get(start..=end)
}
