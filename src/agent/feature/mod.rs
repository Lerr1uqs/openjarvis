//! Feature providers that materialize stable system messages into `Thread` during init.

use super::{memory::ActiveMemoryCatalogFeaturePromptProvider, tool::ToolRegistry};
use crate::{
    compact::ContextBudgetReport,
    config::AgentCompactConfig,
    context::{ChatMessage, ChatMessageRole},
    thread::Thread,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::info;

const TOOL_USE_MODE_PROMPT: &str = "You are running in OpenJarvis tool-use mode. Use the provided tools when needed. You may also provide a short user-visible reply before calling a tool.";

/// Shared immutable input passed to one feature prompt provider rebuild.
pub struct FeaturePromptBuildContext<'a> {
    pub thread_context: &'a Thread,
    pub created_at: DateTime<Utc>,
    pub auto_compact_enabled: bool,
}

/// Fixed provider contract for one thread-scoped init system-message producer.
#[async_trait]
pub trait FeaturePromptProvider: Send + Sync {
    /// Return the stable provider name used for logging and tests.
    fn name(&self) -> &'static str;

    /// Build the stable init-time system messages from immutable thread inputs.
    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>>;
}

/// Build all fixed feature prompts into one init-time system-message snapshot.
pub struct FeaturePromptRebuilder {
    pub providers: Vec<Box<dyn FeaturePromptProvider>>,
}

impl FeaturePromptRebuilder {
    /// Create the fixed provider set used by the agent loop.
    pub fn new(
        tool_registry: Arc<ToolRegistry>,
        compact_config: AgentCompactConfig,
        system_prompt: impl Into<String>,
    ) -> Self {
        let system_prompt = system_prompt.into();
        Self {
            providers: vec![
                Box::new(StaticSystemPromptFeaturePromptProvider::new(system_prompt)),
                Box::new(ToolsetCatalogFeaturePromptProvider::new(Arc::clone(
                    &tool_registry,
                ))),
                Box::new(ActiveMemoryCatalogFeaturePromptProvider::new(
                    tool_registry.memory_repository(),
                )),
                Box::new(SkillCatalogFeaturePromptProvider::new(Arc::clone(
                    &tool_registry,
                ))),
                Box::new(AutoCompactFeaturePromptProvider::new(compact_config)),
            ],
        }
    }

    /// Build the current stable feature messages in fixed provider order.
    pub async fn build_messages(
        &self,
        thread_context: &Thread,
        auto_compact_enabled: bool,
    ) -> Result<Vec<ChatMessage>> {
        let created_at = Utc::now();
        let context = FeaturePromptBuildContext {
            thread_context,
            created_at,
            auto_compact_enabled,
        };
        let mut messages = Vec::new();

        for provider in &self.providers {
            messages.extend(provider.build(&context).await?);
        }

        info!(
            thread_id = %thread_context.locator.thread_id,
            provider_count = self.providers.len(),
            system_message_count = messages.len(),
            "built fixed feature init messages for thread"
        );
        Ok(messages)
    }
}

/// Static system prompt provider that emits the configured worker prompt during thread init.
pub struct StaticSystemPromptFeaturePromptProvider {
    system_prompt: String,
}

impl StaticSystemPromptFeaturePromptProvider {
    pub fn new(system_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: system_prompt.into(),
        }
    }
}

#[async_trait]
impl FeaturePromptProvider for StaticSystemPromptFeaturePromptProvider {
    fn name(&self) -> &'static str {
        "worker_system_prompt"
    }

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>> {
        let system_prompt = self.system_prompt.trim();
        if system_prompt.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![ChatMessage::new(
            ChatMessageRole::System,
            system_prompt,
            context.created_at,
        )])
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

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>> {
        let mut messages = vec![ChatMessage::new(
            ChatMessageRole::System,
            TOOL_USE_MODE_PROMPT,
            context.created_at,
        )];
        if let Some(catalog_prompt) = context
            .thread_context
            .toolset_catalog_prompt_with_registry(&self.tool_registry)
            .await
        {
            messages.push(ChatMessage::new(
                ChatMessageRole::System,
                catalog_prompt,
                context.created_at,
            ));
        }

        Ok(messages)
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

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>> {
        Ok(self
            .tool_registry
            .skills()
            .catalog_prompt()
            .await
            .into_iter()
            .map(|catalog_prompt| {
                ChatMessage::new(ChatMessageRole::System, catalog_prompt, context.created_at)
            })
            .collect::<Vec<_>>())
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

    async fn build(&self, context: &FeaturePromptBuildContext<'_>) -> Result<Vec<ChatMessage>> {
        if !context.auto_compact_enabled {
            return Ok(Vec::new());
        }

        Ok(vec![ChatMessage::new(
            ChatMessageRole::System,
            "Auto-compact 已开启。当当前线程预算达到阈值时，系统会向你暴露 `compact` 工具；当你判断剩余上下文不足以安全继续时，可以主动调用它。`compact` 只会压缩当前线程的非 system 历史。",
            context.created_at,
        )])
    }
}

/// Runtime auto-compact helper that decides compact visibility and runtime thresholds from the
/// current request budget.
pub struct AutoCompactor {
    compact_config: AgentCompactConfig,
}

impl AutoCompactor {
    pub fn new(compact_config: AgentCompactConfig) -> Self {
        Self { compact_config }
    }

    /// Return whether the current request budget should expose the `compact` tool.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     agent::AutoCompactor,
    ///     compact::ContextBudgetReport,
    ///     config::AgentCompactConfig,
    ///     context::ContextTokenKind,
    /// };
    /// use std::collections::HashMap;
    ///
    /// let budget_report = ContextBudgetReport::new(
    ///     HashMap::from([
    ///         (ContextTokenKind::System, 32),
    ///         (ContextTokenKind::Chat, 180),
    ///         (ContextTokenKind::VisibleTool, 16),
    ///         (ContextTokenKind::ReservedOutput, 16),
    ///     ]),
    ///     256,
    /// );
    ///
    /// assert!(AutoCompactor::new(AgentCompactConfig::default())
    ///     .compact_tool_visible(&budget_report));
    /// ```
    pub fn compact_tool_visible(&self, budget_report: &ContextBudgetReport) -> bool {
        budget_report.reaches_ratio(self.compact_config.tool_visible_threshold_ratio())
    }

    /// Return whether runtime compact should run before the next generate.
    pub fn runtime_compaction_required(&self, budget_report: &ContextBudgetReport) -> bool {
        budget_report.reaches_ratio(self.compact_config.runtime_threshold_ratio())
    }
}
