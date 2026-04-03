//! Feature-prompt providers that rebuild fixed feature system prompts on `ThreadContext`.

use super::tool::ToolRegistry;
use crate::{
    compact::ContextBudgetReport,
    config::AgentCompactConfig,
    context::{ChatMessage, ChatMessageRole},
    thread::{ThreadContext, ThreadFeaturesSystemPrompt},
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::info;

const TOOL_USE_MODE_PROMPT: &str = "You are running in OpenJarvis tool-use mode. Use the provided tools when needed. You may also provide a short user-visible reply before calling a tool.";

/// Shared immutable input passed to one feature prompt provider rebuild.
pub struct FeaturePromptBuildContext<'a> {
    pub thread_context: &'a ThreadContext,
    pub created_at: DateTime<Utc>,
    pub auto_compact_enabled: bool,
}

/// Fixed provider contract for one thread-scoped dynamic feature prompt.
#[async_trait]
pub trait FeaturePromptProvider: Send + Sync {
    /// Return the stable provider name used for logging and tests.
    fn name(&self) -> &'static str;

    /// Build the current feature prompt output from immutable thread/runtime inputs.
    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<FeaturePromptOutput>;
}

/// One provider rebuild result mapped back into `ThreadContext`.
pub enum FeaturePromptOutput {
    ToolsetCatalog(Vec<ChatMessage>),
    SkillCatalog(Vec<ChatMessage>),
    AutoCompact(Vec<ChatMessage>),
}

impl FeaturePromptOutput {
    fn write_into(self, features_system_prompt: &mut ThreadFeaturesSystemPrompt) {
        match self {
            Self::ToolsetCatalog(messages) => {
                features_system_prompt.toolset_catalog = messages;
            }
            Self::SkillCatalog(messages) => {
                features_system_prompt.skill_catalog = messages;
            }
            Self::AutoCompact(messages) => {
                features_system_prompt.auto_compact = messages;
            }
        }
    }
}

/// Rebuild all fixed feature prompt providers into the current `ThreadContext`.
pub struct FeaturePromptRebuilder {
    pub providers: Vec<Box<dyn FeaturePromptProvider>>,
}

impl FeaturePromptRebuilder {
    /// Create the fixed provider set used by the agent loop.
    pub fn new(tool_registry: Arc<ToolRegistry>, compact_config: AgentCompactConfig) -> Self {
        Self {
            providers: vec![
                Box::new(ToolsetCatalogFeaturePromptProvider::new(Arc::clone(
                    &tool_registry,
                ))),
                Box::new(SkillCatalogFeaturePromptProvider::new(Arc::clone(
                    &tool_registry,
                ))),
                Box::new(AutoCompactFeaturePromptProvider::new(compact_config)),
            ],
        }
    }

    /// Rebuild the current thread feature system prompt in fixed provider order.
    pub async fn rebuild(
        &self,
        thread_context: &mut ThreadContext,
        auto_compact_enabled: bool,
    ) -> Result<()> {
        let created_at = Utc::now();
        let context = FeaturePromptBuildContext {
            thread_context,
            created_at,
            auto_compact_enabled,
        };
        let mut features_system_prompt = ThreadFeaturesSystemPrompt::default();

        for provider in &self.providers {
            let output = provider.build(&context).await?;
            output.write_into(&mut features_system_prompt);
        }

        info!(
            thread_id = %thread_context.locator.thread_id,
            provider_count = self.providers.len(),
            "rebuilding fixed feature prompt providers for thread"
        );
        thread_context.rebuild_features_system_prompt(features_system_prompt);
        Ok(())
    }
}

/// Toolset catalog provider for fixed feature system prompt slots.
pub struct ToolsetCatalogFeaturePromptProvider {
    tool_registry: Arc<ToolRegistry>,
}

impl ToolsetCatalogFeaturePromptProvider {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self { tool_registry }
    }
}

#[async_trait]
impl FeaturePromptProvider for ToolsetCatalogFeaturePromptProvider {
    fn name(&self) -> &'static str {
        "toolset_catalog"
    }

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<FeaturePromptOutput> {
        let mut messages = vec![ChatMessage::new(
            ChatMessageRole::System,
            TOOL_USE_MODE_PROMPT,
            context.created_at,
        )];
        if let Some(catalog_prompt) = self
            .tool_registry
            .catalog_prompt_for_context(context.thread_context)
            .await
        {
            messages.push(ChatMessage::new(
                ChatMessageRole::System,
                catalog_prompt,
                context.created_at,
            ));
        }

        Ok(FeaturePromptOutput::ToolsetCatalog(messages))
    }
}

/// Skill catalog provider for fixed feature system prompt slots.
pub struct SkillCatalogFeaturePromptProvider {
    tool_registry: Arc<ToolRegistry>,
}

impl SkillCatalogFeaturePromptProvider {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self { tool_registry }
    }
}

#[async_trait]
impl FeaturePromptProvider for SkillCatalogFeaturePromptProvider {
    fn name(&self) -> &'static str {
        "skill_catalog"
    }

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<FeaturePromptOutput> {
        let messages = self
            .tool_registry
            .skills()
            .catalog_prompt()
            .await
            .into_iter()
            .map(|catalog_prompt| {
                ChatMessage::new(ChatMessageRole::System, catalog_prompt, context.created_at)
            })
            .collect::<Vec<_>>();
        Ok(FeaturePromptOutput::SkillCatalog(messages))
    }
}

/// Auto-compact provider that only emits the stable feature system prompt.
pub struct AutoCompactFeaturePromptProvider {
    _compact_config: AgentCompactConfig, // 为了后续扩展 auto-compact prompt 配置而保留
}

impl AutoCompactFeaturePromptProvider {
    pub fn new(compact_config: AgentCompactConfig) -> Self {
        Self {
            _compact_config: compact_config,
        }
    }
}

#[async_trait]
impl FeaturePromptProvider for AutoCompactFeaturePromptProvider {
    fn name(&self) -> &'static str {
        "auto_compact"
    }

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<FeaturePromptOutput> {
        if !context.auto_compact_enabled {
            return Ok(FeaturePromptOutput::AutoCompact(Vec::new()));
        }

        Ok(FeaturePromptOutput::AutoCompact(vec![ChatMessage::new(
            ChatMessageRole::System,
            "Auto-compact 已开启。`compact` 工具当前可用；当你判断剩余上下文不足以安全继续时，可以主动调用它。`compact` 只会压缩当前线程的 chat 历史，不会压缩 system 或 memory。",
            context.created_at,
        )]))
    }
}

/// Runtime auto-compact notifier that injects transient capacity prompts without allocating one fixed slot.
pub struct AutoCompactor {
    compact_config: AgentCompactConfig,
}

impl AutoCompactor {
    pub fn new(compact_config: AgentCompactConfig) -> Self {
        Self { compact_config }
    }

    /// Refresh the request-time context-capacity prompt for the current thread.
    ///
    /// # 示例
    /// ```rust
    /// use std::collections::HashMap;
    ///
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AutoCompactor,
    ///     compact::ContextBudgetReport,
    ///     config::AgentCompactConfig,
    ///     context::ContextTokenKind,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// let budget_report = ContextBudgetReport::new(
    ///     HashMap::from([
    ///         (ContextTokenKind::System, 32),
    ///         (ContextTokenKind::Chat, 128),
    ///         (ContextTokenKind::VisibleTool, 16),
    ///         (ContextTokenKind::ReservedOutput, 16),
    ///     ]),
    ///     256,
    /// );
    ///
    /// AutoCompactor::new(AgentCompactConfig::default())
    ///     .notify_capacity(&mut thread_context, Some(&budget_report));
    ///
    /// assert!(thread_context
    ///     .messages()
    ///     .iter()
    ///     .any(|message| message.content.contains("<context capacity")));
    /// ```
    pub fn notify_capacity(
        &self,
        thread_context: &mut ThreadContext,
        budget_report: Option<&ContextBudgetReport>,
    ) {
        let Some(budget_report) = budget_report else {
            thread_context.replace_live_system_messages(Vec::new());
            return;
        };

        thread_context.replace_live_system_messages(vec![ChatMessage::new(
            ChatMessageRole::System,
            build_auto_compact_dynamic_prompt(
                budget_report,
                self.compact_config.tool_visible_threshold_ratio(),
                self.compact_config.runtime_threshold_ratio(),
            ),
            Utc::now(),
        )]);
    }
}

fn build_auto_compact_dynamic_prompt(
    budget_report: &ContextBudgetReport,
    tool_visible_threshold_ratio: f64,
    runtime_threshold_ratio: f64,
) -> String {
    let token_breakdown = crate::context::ContextTokenKind::ALL
        .into_iter()
        .map(|kind| format!("{}={}", kind.as_str(), budget_report.tokens(kind)))
        .collect::<Vec<_>>()
        .join(", ");
    let utilization_percent = budget_report.utilization_ratio * 100.0;
    let soft_threshold_percent = tool_visible_threshold_ratio * 100.0;
    let runtime_threshold_percent = runtime_threshold_ratio * 100.0;
    let guidance = if budget_report.reaches_ratio(runtime_threshold_ratio) {
        format!(
            "当前上下文占用已经接近 runtime compact 阈值 ({runtime_threshold_percent:.1}%)，如果你还需要继续消耗上下文，应优先调用 `compact`。"
        )
    } else if budget_report.reaches_ratio(tool_visible_threshold_ratio) {
        format!(
            "当前上下文占用已经超过 auto_compact 提前提醒阈值 ({soft_threshold_percent:.1}%)，应主动考虑尽快调用 `compact`。"
        )
    } else {
        "如果你判断剩余上下文不足以安全继续，可以主动调用 `compact`。".to_string()
    };

    format!(
        "<context capacity {utilization_percent:.1}% used>\ncurrent_context_budget: {token_breakdown}, total_estimated_tokens={total_estimated_tokens}, context_window_tokens={context_window_tokens}, utilization_ratio={utilization_ratio:.3}, soft_threshold={tool_visible_threshold_ratio:.3}, runtime_threshold={runtime_threshold_ratio:.3}.\n{guidance}",
        utilization_percent = utilization_percent,
        token_breakdown = token_breakdown,
        total_estimated_tokens = budget_report.total_estimated_tokens,
        context_window_tokens = budget_report.context_window_tokens,
        utilization_ratio = budget_report.utilization_ratio,
        tool_visible_threshold_ratio = tool_visible_threshold_ratio,
        runtime_threshold_ratio = runtime_threshold_ratio,
        guidance = guidance,
    )
}
