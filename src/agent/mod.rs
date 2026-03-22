//! Agent-layer modules for the loop, runtime, hooks, tools, and worker orchestration.

pub mod agent_loop;
pub mod hook;
pub mod mcp;
pub mod runtime;
pub mod tool;
pub mod worker;

pub use agent_loop::{
    AgentEventSender, AgentLoop, AgentLoopEvent, AgentLoopEventKind, AgentLoopOutput, InfoContext,
};
pub use hook::{HookEvent, HookEventKind, HookHandler, HookRegistry};
pub use mcp::{McpRegistry, McpServerDefinition, McpTransport};
pub use runtime::AgentRuntime;
pub use tool::{
    EditTool, ReadTool, ShellTool, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    ToolInputSchema, ToolRegistry, ToolSchemaProtocol, WriteTool,
};
pub use worker::AgentWorker;
