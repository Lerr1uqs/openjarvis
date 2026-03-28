//! Context-budget estimation for compact and auto-compact.
//!
//! The estimator is intentionally deterministic in V1. It does not try to match any provider's
//! exact tokenizer output; instead it provides a stable request-level approximation that runtime
//! compact and auto-compact can share.
//! TODO: 这个是和default LLM强绑定的 可能需要建立某种绑定关系

use crate::{
    agent::ToolDefinition,
    config::{AgentCompactConfig, LLMConfig},
    context::{ChatMessage, ContextTokenKind},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Currently supported deterministic token estimator name.
pub const CHARS_DIV4_TOKENIZER: &str = "chars_div4";

/// One request-level context budget report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextBudgetReport {
    // 现在很多地方把 budget_report 当普通 JSON 用
    #[serde(flatten)]
    token_counts: HashMap<ContextTokenKind, usize>,
    pub total_estimated_tokens: usize,
    pub context_window_tokens: usize,
    pub utilization_ratio: f64,
}

impl ContextBudgetReport {
    /// Build one request-level budget report from aligned token buckets.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     compact::ContextBudgetReport,
    ///     context::ContextTokenKind,
    /// };
    /// use std::collections::HashMap;
    ///
    /// let report = ContextBudgetReport::new(
    ///     HashMap::from([
    ///         (ContextTokenKind::System, 10),
    ///         (ContextTokenKind::Chat, 40),
    ///         (ContextTokenKind::ReservedOutput, 16),
    ///     ]),
    ///     128,
    /// );
    ///
    /// assert_eq!(report.total_estimated_tokens, 66);
    /// ```
    pub fn new(
        token_counts: HashMap<ContextTokenKind, usize>,
        context_window_tokens: usize,
    ) -> Self {
        let mut report = Self {
            token_counts,
            total_estimated_tokens: 0,
            context_window_tokens,
            utilization_ratio: 0.0,
        };
        report.recalculate_totals();
        report
    }

    /// Return the aligned token-count map keyed by `ContextTokenKind`.
    pub fn token_counts(&self) -> &HashMap<ContextTokenKind, usize> {
        &self.token_counts
    }

    /// Return the token count for one request-budget bucket.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     compact::ContextBudgetReport,
    ///     context::ContextTokenKind,
    /// };
    /// use std::collections::HashMap;
    ///
    /// let report = ContextBudgetReport::new(
    ///     HashMap::from([
    ///         (ContextTokenKind::System, 10),
    ///         (ContextTokenKind::Memory, 5),
    ///         (ContextTokenKind::Chat, 40),
    ///         (ContextTokenKind::VisibleTool, 12),
    ///         (ContextTokenKind::ReservedOutput, 16),
    ///     ]),
    ///     128,
    /// );
    ///
    /// assert_eq!(report.tokens(ContextTokenKind::Chat), 40);
    /// ```
    pub fn tokens(&self, kind: ContextTokenKind) -> usize {
        self.token_counts.get(&kind).copied().unwrap_or(0)
    }

    /// Return the estimated system token count.
    pub fn system_tokens(&self) -> usize {
        self.tokens(ContextTokenKind::System)
    }

    /// Return the estimated memory token count.
    pub fn memory_tokens(&self) -> usize {
        self.tokens(ContextTokenKind::Memory)
    }

    /// Return the estimated chat token count.
    pub fn chat_tokens(&self) -> usize {
        self.tokens(ContextTokenKind::Chat)
    }

    /// Return the estimated visible-tool token count.
    pub fn visible_tool_tokens(&self) -> usize {
        self.tokens(ContextTokenKind::VisibleTool)
    }

    /// Return the reserved output token count.
    pub fn reserved_output_tokens(&self) -> usize {
        self.tokens(ContextTokenKind::ReservedOutput)
    }

    /// Return whether the report crosses the runtime compact threshold.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{compact::ContextBudgetReport, context::ContextTokenKind};
    /// use std::collections::HashMap;
    ///
    /// let report = ContextBudgetReport::new(
    ///     HashMap::from([
    ///         (ContextTokenKind::System, 10),
    ///         (ContextTokenKind::Chat, 50),
    ///         (ContextTokenKind::VisibleTool, 10),
    ///         (ContextTokenKind::ReservedOutput, 10),
    ///     ]),
    ///     100,
    /// );
    ///
    /// assert!(!report.reaches_ratio(0.9));
    /// assert!(report.reaches_ratio(0.8));
    /// ```
    pub fn reaches_ratio(&self, ratio: f64) -> bool {
        self.utilization_ratio >= ratio
    }

    fn set_tokens(&mut self, kind: ContextTokenKind, tokens: usize) {
        self.token_counts.insert(kind, tokens);
    }

    fn add_tokens(&mut self, kind: ContextTokenKind, tokens: usize) {
        self.set_tokens(kind, self.tokens(kind) + tokens);
    }

    fn recalculate_totals(&mut self) {
        self.total_estimated_tokens = ContextTokenKind::ALL
            .into_iter()
            .map(|kind| self.tokens(kind))
            .sum::<usize>();
        self.utilization_ratio =
            self.total_estimated_tokens as f64 / self.context_window_tokens.max(1) as f64;
    }
}

/// Deterministic estimator used by runtime compact and auto-compact.
pub struct ContextBudgetEstimator {
    context_window_tokens: usize,
    reserved_output_tokens: usize,
    tokenizer: String,
}

impl ContextBudgetEstimator {
    /// Build one estimator from loaded config values.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{compact::ContextBudgetEstimator, config::AppConfig};
    ///
    /// let config = AppConfig::default();
    /// let estimator = ContextBudgetEstimator::from_config(
    ///     config.llm_config(),
    ///     config.agent_config().compact_config(),
    /// );
    ///
    /// assert_eq!(estimator.context_window_tokens(), 8192);
    /// ```
    pub fn from_config(llm: &LLMConfig, compact: &AgentCompactConfig) -> Self {
        Self {
            context_window_tokens: llm.context_window_tokens(),
            reserved_output_tokens: llm
                .max_output_tokens
                .or_else(|| compact.configured_reserved_output_tokens())
                .unwrap_or_else(|| llm.max_output_tokens()),
            tokenizer: llm.tokenizer.clone(),
        }
    }

    /// Return the configured context window tokens.
    pub fn context_window_tokens(&self) -> usize {
        self.context_window_tokens
    }

    /// Estimate the full request-level budget.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     compact::ContextBudgetEstimator,
    ///     config::AppConfig,
    ///     context::{ChatMessage, ChatMessageRole},
    /// };
    ///
    /// let config = AppConfig::default();
    /// let estimator = ContextBudgetEstimator::from_config(
    ///     config.llm_config(),
    ///     config.agent_config().compact_config(),
    /// );
    /// let report = estimator.estimate(
    ///     &[ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())],
    ///     &[],
    /// );
    ///
    /// assert!(report.chat_tokens() > 0);
    /// assert_eq!(report.visible_tool_tokens(), 0);
    /// ```
    pub fn estimate(
        &self,
        messages: &[ChatMessage],
        visible_tools: &[ToolDefinition],
    ) -> ContextBudgetReport {
        let mut report = ContextBudgetReport::new(HashMap::new(), self.context_window_tokens);

        for message in messages {
            let estimated_tokens = self.estimate_message_tokens(message);
            report.add_tokens(
                ContextTokenKind::for_chat_message_role(&message.role),
                estimated_tokens,
            );
        }

        let visible_tool_tokens = visible_tools
            .iter()
            .map(|tool| self.estimate_tool_tokens(tool))
            .sum::<usize>();
        report.set_tokens(ContextTokenKind::VisibleTool, visible_tool_tokens);
        report.set_tokens(
            ContextTokenKind::ReservedOutput,
            self.reserved_output_tokens,
        );
        report.recalculate_totals();

        report
    }

    fn estimate_message_tokens(&self, message: &ChatMessage) -> usize {
        let role_overhead = 4;
        let content_tokens = self.estimate_text_tokens(&message.content);
        let tool_calls_tokens = if message.tool_calls.is_empty() {
            0
        } else {
            self.estimate_text_tokens(
                &serde_json::to_string(&message.tool_calls)
                    .expect("chat tool calls should serialize for budget estimation"),
            )
        };
        let tool_call_id_tokens = message
            .tool_call_id
            .as_deref()
            .map(|value| self.estimate_text_tokens(value))
            .unwrap_or(0);

        role_overhead + content_tokens + tool_calls_tokens + tool_call_id_tokens
    }

    fn estimate_tool_tokens(&self, tool: &ToolDefinition) -> usize {
        let tool_overhead = 8;
        let schema_tokens = self.estimate_text_tokens(
            &serde_json::to_string(tool.input_schema.json_schema())
                .expect("tool schema should serialize for budget estimation"),
        );

        tool_overhead
            + self.estimate_text_tokens(&tool.name)
            + self.estimate_text_tokens(&tool.description)
            + schema_tokens
    }

    fn estimate_text_tokens(&self, text: &str) -> usize {
        match self.tokenizer.as_str() {
            CHARS_DIV4_TOKENIZER => estimate_chars_div4_tokens(text),
            _ => estimate_chars_div4_tokens(text),
        }
    }
}

fn estimate_chars_div4_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    if char_count == 0 {
        return 0;
    }

    char_count.div_ceil(4)
}
