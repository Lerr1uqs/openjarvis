mod feature;
mod repository;
mod tool;

use openjarvis::{
    agent::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolRegistry},
    thread::{Thread, ThreadContextLocator},
};
use std::{
    env::temp_dir,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(crate) struct MemoryWorkspaceFixture {
    root: PathBuf,
}

impl MemoryWorkspaceFixture {
    pub(crate) fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("memory workspace root should be created");
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn memory_root(&self) -> PathBuf {
        self.root.join(".openjarvis/memory")
    }
}

impl Drop for MemoryWorkspaceFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

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
