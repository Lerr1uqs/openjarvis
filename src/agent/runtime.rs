//! Shared runtime container that holds hooks, tools, and MCP registries for the agent.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, hook::HookRegistry, tool::ToolRegistry,
};
use crate::{
    config::{AgentConfig, global_config},
    thread::Thread,
};
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct AgentRuntime {
    hooks: Arc<HookRegistry>,
    tools: Arc<ToolRegistry>,
}

impl AgentRuntime {
    /// Create a runtime with empty hook, tool, and MCP registries.
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(HookRegistry::new()),
            tools: Arc::new(ToolRegistry::new()),
        }
    }

    /// Create a runtime from the loaded `agent` config section.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{agent::AgentRuntime, config::AppConfig};
    ///
    /// let config = AppConfig::default();
    /// let runtime = AgentRuntime::from_config(config.agent_config()).await?;
    /// assert_eq!(runtime.hooks().len().await, 0);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &AgentConfig) -> Result<Self> {
        Self::from_config_with_skill_roots(config, vec![PathBuf::from(".skills")]).await
    }

    /// Create a runtime from the installed global app config snapshot.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     config::{AppConfig, install_global_config},
    /// };
    ///
    /// let config = AppConfig::builder_for_test().build()?;
    /// install_global_config(config)?;
    ///
    /// let runtime = AgentRuntime::from_global_config().await?;
    /// assert_eq!(runtime.hooks().len().await, 0);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_global_config() -> Result<Self> {
        Self::from_config(global_config().agent_config()).await
    }

    /// Create a runtime from config with explicit local skill roots.
    ///
    /// This exists mainly so tests can opt into deterministic roots instead of using the
    /// workspace `.skills` directory.
    pub async fn from_config_with_skill_roots(
        config: &AgentConfig,
        skill_roots: Vec<PathBuf>,
    ) -> Result<Self> {
        Ok(Self {
            hooks: Arc::new(
                HookRegistry::from_config(config.hook_config())
                    .await?
            ),
            tools: Arc::new(
                ToolRegistry::from_config_with_skill_roots(
                    config.tool_config(), 
                    skill_roots
                ).await?,
            ),
        })
    }

    pub fn with_parts(hooks: Arc<HookRegistry>, tools: Arc<ToolRegistry>) -> Self {
        Self { hooks, tools }
    }

    /// Return the shared hook registry.
    pub fn hooks(&self) -> Arc<HookRegistry> {
        Arc::clone(&self.hooks)
    }

    /// Return the shared tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// List visible tools for the current thread and request visibility mode.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     thread::{Thread, ThreadContextLocator},
    /// };
    ///
    /// let runtime = AgentRuntime::new();
    /// let thread_context = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// let _tools = runtime.list_tools(&thread_context, false).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_tools(
        &self,
        thread_context: &Thread,
        compact_visible: bool,
    ) -> Result<Vec<ToolDefinition>> {
        self.tools
            .list_for_context_with_compact(thread_context, compact_visible)
            .await
    }

    /// Open one optional tool entry for the current thread.
    pub async fn open_tool(&self, thread_context: &mut Thread, tool_name: &str) -> Result<bool> {
        self.tools.open_tool(thread_context, tool_name).await
    }

    /// Close one optional tool entry for the current thread.
    pub async fn close_tool(&self, thread_context: &mut Thread, tool_name: &str) -> Result<bool> {
        self.tools.close_tool(thread_context, tool_name).await
    }

    /// Execute one runtime-managed tool call inside the current thread.
    pub async fn call_tool(
        &self,
        thread_context: &mut Thread,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        self.tools.call_for_context(thread_context, request).await
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
