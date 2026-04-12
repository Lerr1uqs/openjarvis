//! Agent-layer modules for the loop, runtime, hooks, tools, and worker orchestration.

pub mod agent_loop;
pub mod feature;
pub mod hook;
pub mod memory;
pub mod runtime;
pub mod sandbox;
pub mod tool;
pub mod worker;

pub use agent_loop::{
    AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEvent, AgentLoopEventKind,
    AgentLoopOutput,
};
pub use feature::{AutoCompactor, FeatureResolver, ShellEnv};
pub use hook::{HookEvent, HookEventKind, HookHandler, HookRegistry};
pub use memory::{
    ActiveMemoryCatalogEntry, MemoryDocument, MemoryDocumentMetadata, MemoryDocumentSummary,
    MemoryRepository, MemorySearchResponse, MemoryType, MemoryWriteRequest,
    register_memory_toolset,
};
pub use runtime::AgentRuntime;
pub use sandbox::DummySandboxContainer;
pub use tool::{
    CommandExecutionRequest, CommandExecutionResult, CommandSessionManager, CommandTaskSnapshot,
    CommandTaskStatus, CommandWriteRequest, EditTool, ExecCommandTool, ListUnreadCommandTasksTool,
    LoadSkillTool, LoadedSkill, LoadedSkillFile, McpServerDefinition, McpServerSnapshot,
    McpServerState, McpToolSnapshot, McpTransport, ReadTool, ShellTool, SkillManifest,
    SkillRegistry, ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    ToolInputSchema, ToolRegistry, ToolSchemaProtocol, ToolSource, ToolSourceMcp,
    ToolsetCatalogEntry, ToolsetRuntime, WriteStdinTool, WriteTool, empty_tool_input_schema,
};
pub use worker::{
    AgentRequest, AgentWorker, AgentWorkerBuilder, AgentWorkerEvent, AgentWorkerHandle,
    CommittedAgentDispatchItem, CompletedAgentRequest,
};
