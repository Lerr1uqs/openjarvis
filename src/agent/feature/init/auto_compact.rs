//! Stable initialization helpers and runtime threshold decisions for `Feature::AutoCompact`.

use crate::{compact::ContextBudgetReport, config::AgentCompactConfig};

const AUTO_COMPACT_USAGE_PROMPT: &str = "Auto-compact 已开启。当当前线程预算达到阈值时，系统会向你暴露 `compact` 工具；当你判断剩余上下文不足以安全继续时，可以主动调用它。`compact` 只会压缩当前线程的非 system 历史。";
const AUTO_COMPACT_TOOL_VISIBILITY_PROMPT: &str = "当 `compact` 工具未出现在当前工具列表中时，表示当前上下文预算还没有达到允许你主动压缩的阈值；只有工具真正可见时你才能调用它。";

/// Return whether runtime config currently allows `Feature::AutoCompact` to exist.
pub fn is_available(compact_config: &AgentCompactConfig) -> bool {
    compact_config.enabled() && compact_config.auto_compact()
}

/// Return the stable usage prompt when auto-compact is available for this runtime.
pub fn usage(compact_config: &AgentCompactConfig) -> Option<&'static str> {
    is_available(compact_config).then_some(AUTO_COMPACT_USAGE_PROMPT)
}

/// Return the stable tool-visibility prompt for the auto-compact feature.
pub fn tool_visibility_prompt(compact_config: &AgentCompactConfig) -> Option<&'static str> {
    is_available(compact_config).then_some(AUTO_COMPACT_TOOL_VISIBILITY_PROMPT)
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
