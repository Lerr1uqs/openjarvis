//! Shared runtime container that holds hooks, tools, compact runtime switches, and MCP registries for one agent.

use super::{
    ToolCallRequest, ToolCallResult, ToolDefinition, hook::HookRegistry, tool::ToolRegistry,
};
use crate::config::AgentConfig;
use crate::{
    compact::{CompactRuntimeManager, CompactScopeKey},
    thread::ThreadContext,
};
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct AgentRuntime {
    hooks: Arc<HookRegistry>,
    tools: Arc<ToolRegistry>,
    compact_runtime: Arc<CompactRuntimeManager>,
}

impl AgentRuntime {
    /// Create a runtime with empty hook, tool, and MCP registries.
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(HookRegistry::new()),
            tools: Arc::new(ToolRegistry::new()),
            compact_runtime: Arc::new(CompactRuntimeManager::new()),
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
            compact_runtime: Arc::new(CompactRuntimeManager::new()),
        })
    }

    pub fn with_parts(hooks: Arc<HookRegistry>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            hooks,
            tools,
            compact_runtime: Arc::new(CompactRuntimeManager::new()),
        }
    }

    /// Return the shared hook registry.
    pub fn hooks(&self) -> Arc<HookRegistry> {
        Arc::clone(&self.hooks)
    }

    /// Return the shared tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }

    /// Return the shared compact runtime override manager.
    pub fn compact_runtime(&self) -> Arc<CompactRuntimeManager> {
        Arc::clone(&self.compact_runtime)
    }

    /// Merge runtime-managed thread state into one `ThreadContext`.
    ///
    /// This restores legacy tool runtime state and compact overrides before a loop turn starts.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     compact::CompactScopeKey,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let runtime = AgentRuntime::new();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// let scope = CompactScopeKey::new("feishu", "ou_xxx", "thread_ext");
    ///
    /// runtime.merge_thread_state(&scope, &mut thread_context).await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn merge_thread_state(
        &self,
        compact_scope_key: &CompactScopeKey,
        thread_context: &mut ThreadContext,
    ) {
        self.tools.merge_legacy_thread_state(thread_context).await;
        self.compact_runtime
            .merge_legacy_scope_overrides(compact_scope_key, thread_context)
            .await;
    }

    /// List visible tools for the current thread and request visibility mode.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let runtime = AgentRuntime::new();
    /// let thread_context = ThreadContext::new(
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
        thread_context: &ThreadContext,
        compact_visible: bool,
    ) -> Result<Vec<ToolDefinition>> {
        self.tools
            .list_for_context_with_compact(thread_context, compact_visible)
            .await
    }

    /// Open one optional tool entry for the current thread.
    ///
    /// At the moment this maps to loading one named toolset.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let runtime = AgentRuntime::new();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// let _opened = runtime.open_tool(&mut thread_context, "browser").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn open_tool(
        &self,
        thread_context: &mut ThreadContext,
        tool_name: &str,
    ) -> Result<bool> {
        self.tools.open_tool(thread_context, tool_name).await
    }

    /// Close one optional tool entry for the current thread.
    ///
    /// At the moment this maps to unloading one named toolset.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::AgentRuntime,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let runtime = AgentRuntime::new();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// let _closed = runtime.close_tool(&mut thread_context, "browser").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn close_tool(
        &self,
        thread_context: &mut ThreadContext,
        tool_name: &str,
    ) -> Result<bool> {
        self.tools.close_tool(thread_context, tool_name).await
    }

    /// Execute one runtime-managed tool call inside the current thread.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{AgentRuntime, ToolCallRequest},
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    /// use serde_json::json;
    ///
    /// let runtime = AgentRuntime::new();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// let _ = runtime
    ///     .call_tool(
    ///         &mut thread_context,
    ///         ToolCallRequest {
    ///             name: "load_toolset".to_string(),
    ///             arguments: json!({ "name": "browser" }),
    ///         },
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn call_tool(
        &self,
        thread_context: &mut ThreadContext,
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
