//! Shared runtime container that holds hooks, tools, and MCP registries for one agent.

use super::{hook::HookRegistry, tool::ToolRegistry};
use crate::config::AgentConfig;
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

    /// Create a runtime from config with explicit local skill roots.
    ///
    /// This exists mainly so tests can opt into deterministic roots instead of using the
    /// workspace `.skills` directory.
    pub async fn from_config_with_skill_roots(
        config: &AgentConfig,
        skill_roots: Vec<PathBuf>,
    ) -> Result<Self> {
        Ok(Self {
            hooks: Arc::new(HookRegistry::from_config(config.hook_config()).await?),
            tools: Arc::new(
                ToolRegistry::from_config_with_skill_roots(config.tool_config(), skill_roots)
                    .await?,
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
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}
