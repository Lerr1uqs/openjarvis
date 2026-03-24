//! Shared runtime container that holds hooks, tools, and MCP registries for one agent.

use super::{hook::HookRegistry, mcp::McpRegistry, tool::ToolRegistry};
use crate::config::AgentConfig;
use anyhow::Result;
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentRuntime {
    hooks: Arc<HookRegistry>,
    tools: Arc<ToolRegistry>,
    mcp: Arc<McpRegistry>,
}

impl AgentRuntime {
    /// Create a runtime with empty hook, tool, and MCP registries.
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(HookRegistry::new()),
            tools: Arc::new(ToolRegistry::new()),
            mcp: Arc::new(McpRegistry::new()),
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
        Ok(Self {
            hooks: Arc::new(HookRegistry::from_config(config.hook_config()).await?),
            tools: Arc::new(ToolRegistry::new()),
            mcp: Arc::new(McpRegistry::new()),
        })
    }

    pub fn with_parts(
        hooks: Arc<HookRegistry>,
        tools: Arc<ToolRegistry>,
        mcp: Arc<McpRegistry>,
    ) -> Self {
        Self { hooks, tools, mcp }
    }

    /// Return the shared hook registry.
    pub fn hooks(&self) -> Arc<HookRegistry> {
        Arc::clone(&self.hooks)
    }

    /// Return the shared tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// Return the shared MCP registry.
    pub fn mcp(&self) -> Arc<McpRegistry> {
        Arc::clone(&self.mcp)
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
