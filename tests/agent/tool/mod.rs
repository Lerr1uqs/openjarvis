mod browser;
mod command;
mod edit;
pub(crate) mod mcp;
mod obswiki;
mod read;
mod schema;
mod shell;
pub(crate) mod skill;
mod subagent;
mod toolset;
mod write;

use openjarvis::{
    agent::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolRegistry},
    thread::{Thread, ThreadContextLocator},
};

pub(crate) fn build_thread(thread_id: &str) -> Thread {
    Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", thread_id, thread_id),
        chrono::Utc::now(),
    )
}

pub(crate) async fn list_tools(
    registry: &ToolRegistry,
    thread_context: &Thread,
) -> anyhow::Result<Vec<ToolDefinition>> {
    registry.list_for_context(thread_context).await
}

pub(crate) async fn call_tool(
    registry: &ToolRegistry,
    thread_context: &mut Thread,
    request: ToolCallRequest,
) -> anyhow::Result<ToolCallResult> {
    registry.call_for_context(thread_context, request).await
}
