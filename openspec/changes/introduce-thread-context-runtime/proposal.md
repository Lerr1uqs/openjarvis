## Why

当前项目已经出现明显的线程级事务分散问题：`loaded_toolsets`、`compact/auto_compact` 开关、线程工具显隐、未来的审批权限等状态分散在 `ConversationThread`、`ToolRegistry`、`CompactRuntimeManager` 和各个工具实现内部。这样会让线程边界变得模糊，也让 `ToolRegistry` 背负了不属于全局目录层的 thread runtime 责任。

现在需要把线程级状态重新收口到统一的 `ThreadContext` 中，让 AgentLoop、Command 和工具调用都围绕同一个线程上下文工作，同时用渐进迁移而不是激进删除的方式完成重构。

## What Changes

- 引入 `ThreadContext`，由它持有线程定位信息、`ThreadConversation` 和线程级 `ThreadState`。
- 明确线程身份规范：`IncomingMessage.external_thread_id` 只表示外部线程标识；`thread_key = user:channel:external_thread_id`；internal thread id 由该 key 稳定派生；系统不再引入单独的 conversation id。
- 将当前分散在 `ToolRegistry`、`CompactRuntimeManager` 和各类工具内部的 thread-scoped 状态迁移到 `ThreadContext` 管理。
- 将 `ToolRegistry` 收敛为全局工具目录与 handler 解析层，不再作为线程事务管理器。
- 调整 AgentLoop 的主循环输入，让循环内部通过 `ThreadContext` 完成工具可见性计算、工具调用分发、feature 状态读取与事件记录。
- 调整 Command 的处理路径，使所有命令都先解析目标线程，再基于目标 `ThreadContext` 执行，而不是写入独立的全局 override 容器。
- 为现有 thread 相关 API 增加 Rust `#[deprecated]` 标记，保留兼容层，按迁移节奏逐步收敛调用点。
- 同步更新架构文档，明确 `ThreadContext -> ThreadConversation -> ToolRegistry` 的分层关系与迁移策略。

## Capabilities

### New Capabilities
- `thread-context-runtime`: 定义线程上下文作为 thread-scoped runtime 的统一宿主，负责 conversation、feature、工具状态、权限审批和兼容迁移边界。

### Modified Capabilities
- `chat-compact`: 将 compact/auto_compact 的线程级开关与可见性决策迁移到 `ThreadContext`，不再依赖独立的线程 override 管理器。
- `thread-managed-toolsets`: 将线程工具集加载、可见性投影和后续权限策略的所有权从 `ToolRegistry` 调整为 `ThreadContext`。

## Impact

- Affected code: `src/thread.rs`, `src/session.rs`, `src/router.rs`, `src/command.rs`, `src/agent/worker.rs`, `src/agent/agent_loop.rs`, `src/agent/tool/mod.rs`, `src/compact/runtime.rs`
- Affected docs: `arch/system.md`, OpenSpec change/spec artifacts
- API impact: 现有 thread-scoped tool/runtime API 将进入 deprecated 兼容期，新增 `ThreadContext` 相关入口
- Migration impact: 采用“先引入新宿主、再迁移调用点、最后删除旧 API”的渐进重构方式
