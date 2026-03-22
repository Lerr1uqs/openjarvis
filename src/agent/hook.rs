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
    /// 作用: 返回当前 hook handler 的稳定名称，便于日志和调试识别。
    /// 参数: 无，返回 handler 的静态名称。
    fn name(&self) -> &'static str;

    /// 作用: 处理一条 hook 事件，可用于审计、改写或通知。
    /// 参数: event 为 agent loop 在关键节点触发的事件载荷。
    async fn handle(&self, event: &HookEvent) -> Result<()>;
}

#[derive(Default)]
pub struct HookRegistry {
    handlers: RwLock<Vec<Arc<dyn HookHandler>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        // 作用: 创建一个空的 hook registry，用于后续注册各类 hook handler。
        // 参数: 无，初始状态不包含任何 handler。
        Self::default()
    }

    pub async fn register(&self, handler: Arc<dyn HookHandler>) {
        // 作用: 向 registry 中追加一个 hook handler。
        // 参数: handler 为实现 HookHandler trait 的异步处理器。
        self.handlers.write().await.push(handler);
    }

    pub async fn emit(&self, event: HookEvent) -> Result<()> {
        // 作用: 顺序触发所有已注册的 hook handler。
        // 参数: event 为当前 agent 生命周期节点发出的 hook 事件。
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

    pub async fn len(&self) -> usize {
        // 作用: 返回当前已注册 hook handler 的数量。
        // 参数: 无，结果来自 registry 的内存状态。
        self.handlers.read().await.len()
    }
}
