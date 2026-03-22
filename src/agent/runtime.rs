use super::{hook::HookRegistry, mcp::McpRegistry, tool::ToolRegistry};
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentRuntime {
    hooks: Arc<HookRegistry>,
    tools: Arc<ToolRegistry>,
    mcp: Arc<McpRegistry>,
}

impl AgentRuntime {
    pub fn new() -> Self {
        // 作用: 创建默认 agent runtime，包含空的 hook、tool 和 mcp 注册表。
        // 参数: 无，当前用于最小消息闭环的默认运行时。
        Self {
            hooks: Arc::new(HookRegistry::new()),
            tools: Arc::new(ToolRegistry::new()),
            mcp: Arc::new(McpRegistry::new()),
        }
    }

    pub fn with_parts(
        hooks: Arc<HookRegistry>,
        tools: Arc<ToolRegistry>,
        mcp: Arc<McpRegistry>,
    ) -> Self {
        // 作用: 用外部注入的 registries 构造 agent runtime。
        // 参数: hooks 为 hook 注册表，tools 为工具注册表，mcp 为 MCP 注册表。
        Self { hooks, tools, mcp }
    }

    pub fn hooks(&self) -> Arc<HookRegistry> {
        // 作用: 返回 hook registry 的共享引用，便于外部继续注册 hooks。
        // 参数: 无，返回当前 runtime 持有的 hook registry。
        Arc::clone(&self.hooks)
    }

    pub fn tools(&self) -> Arc<ToolRegistry> {
        // 作用: 返回 tool registry 的共享引用，便于外部继续注册工具。
        // 参数: 无，返回当前 runtime 持有的 tool registry。
        Arc::clone(&self.tools)
    }

    pub fn mcp(&self) -> Arc<McpRegistry> {
        // 作用: 返回 MCP registry 的共享引用，便于外部继续注册 MCP 服务。
        // 参数: 无，返回当前 runtime 持有的 MCP registry。
        Arc::clone(&self.mcp)
    }
}

impl Default for AgentRuntime {
    fn default() -> Self {
        // 作用: 为 AgentRuntime 提供默认构造逻辑。
        // 参数: 无，默认行为等同于 AgentRuntime::new。
        Self::new()
    }
}
