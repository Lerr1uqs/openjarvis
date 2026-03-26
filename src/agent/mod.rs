//! Agent-layer modules for the loop, runtime, hooks, tools, and worker orchestration.

pub mod agent_loop;
pub mod hook;
pub mod runtime;
pub mod sandbox;
pub mod tool;
pub mod worker;

pub use agent_loop::{
    AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEvent, AgentLoopEventKind,
    AgentLoopOutput, InfoContext,
};
pub use hook::{HookEvent, HookEventKind, HookHandler, HookRegistry};
pub use runtime::AgentRuntime;
pub use sandbox::DummySandboxContainer;
pub use tool::{
    EditTool, LoadSkillTool, LoadedSkill, LoadedSkillFile, McpServerDefinition, McpServerSnapshot,
    McpServerState, McpToolSnapshot, McpTransport, ReadTool, ShellTool, SkillManifest,
    SkillRegistry, ThreadToolRuntimeManager, ThreadToolRuntimeSnapshot, ToolCallRequest,
    ToolCallResult, ToolDefinition, ToolHandler, ToolInputSchema, ToolRegistry, ToolSchemaProtocol,
    ToolSource, ToolSourceMcp, ToolsetCatalogEntry, WriteTool, empty_tool_input_schema,
};
pub use worker::{
    AgentRequest, AgentWorker, AgentWorkerEvent, AgentWorkerHandle, CompletedAgentTurn,
    FailedAgentTurn,
};
