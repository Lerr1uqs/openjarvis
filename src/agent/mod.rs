//! Agent-layer modules for the loop, runtime, hooks, tools, and worker orchestration.

pub mod agent_loop;
pub mod feature;
pub mod hook;
pub mod runtime;
pub mod sandbox;
pub mod tool;
pub mod worker;

pub use agent_loop::{
    AgentDispatchEvent, AgentEventSender, AgentLoop, AgentLoopEvent, AgentLoopEventKind,
    AgentLoopOutput,
};
pub use feature::{
    AutoCompactFeaturePromptProvider, AutoCompactor, FeaturePromptBuildContext,
    FeaturePromptProvider, FeaturePromptRebuilder, SkillCatalogFeaturePromptProvider,
    ToolsetCatalogFeaturePromptProvider,
};
pub use hook::{HookEvent, HookEventKind, HookHandler, HookRegistry};
pub use runtime::AgentRuntime;
pub use sandbox::DummySandboxContainer;
pub use tool::{
    EditTool, LoadSkillTool, LoadedSkill, LoadedSkillFile, McpServerDefinition, McpServerSnapshot,
    McpServerState, McpToolSnapshot, McpTransport, ReadTool, ShellTool, SkillManifest,
    SkillRegistry, ToolCallContext, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler,
    ToolInputSchema, ToolRegistry, ToolSchemaProtocol, ToolSource, ToolSourceMcp,
    ToolsetCatalogEntry, ToolsetRuntime, WriteTool, empty_tool_input_schema,
};
pub use worker::{
    AgentRequest, AgentWorker, AgentWorkerBuilder, AgentWorkerEvent, AgentWorkerHandle,
    CompletedAgentCommit, FailedAgentCommit, SyncedThreadContext,
};
