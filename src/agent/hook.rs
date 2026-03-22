//! Hook event types and registry used to observe the agent loop lifecycle.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEventKind {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    Stop,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PermissionRequest,
    Notification,
    SessionStart,
    SessionEnd,
    Setup,
    TeammateIdle,
    TaskCompleted,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
}

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub kind: HookEventKind,
    pub payload: Value,
}

#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Return the stable handler name used for logs and diagnostics.
    fn name(&self) -> &'static str;

    /// Handle one hook event emitted by the agent loop.
    async fn handle(&self, event: &HookEvent) -> Result<()>;
}

#[derive(Default)]
pub struct HookRegistry {
    handlers: RwLock<Vec<Arc<dyn HookHandler>>>,
}

impl HookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one hook handler.
    pub async fn register(&self, handler: Arc<dyn HookHandler>) {
        self.handlers.write().await.push(handler);
    }

    /// Emit an event to all registered handlers in registration order.
    pub async fn emit(&self, event: HookEvent) -> Result<()> {
        let handlers = self
            .handlers
            .read()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for handler in handlers {
            handler.handle(&event).await?;
        }

        Ok(())
    }

    /// Return the number of registered handlers.
    pub async fn len(&self) -> usize {
        self.handlers.read().await.len()
    }
}
