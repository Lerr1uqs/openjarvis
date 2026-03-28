## Why

当前 `SessionManager` 只在进程内存中保存 `Session -> ThreadContext`，服务重启后线程历史、线程级 feature 状态和工具加载状态都会丢失。这已经和项目中 `ThreadContext` 作为统一线程事实来源的定位发生冲突，也会让 compact、toolset、命令状态和后续审批能力在重启后失真。

现在需要把线程上下文正式落到可替换的 store 层，并先用 SQLite 提供单机持久化能力，同时为后续切换 PostgreSQL 保留统一接口，避免把数据库细节继续扩散到 `SessionManager`、Router 和 AgentLoop。

## What Changes

- 新增线程上下文持久化能力，定义统一的 `SessionStore` / store trait 边界，由 `SessionManager` 通过该接口读写线程快照。
- 首版提供 SQLite store 实现，用于持久化 `Session` 元数据、`ThreadContext` 快照和必要的去重索引，使重启后可以恢复线程运行态。
- 明确持久化聚合根为 `ThreadContext`，其中会话内容按 `Vec<ConversationTurn>` 持久化，而不是只存扁平 messages 后再反推 turn。
- 明确运行时与持久化边界：`pending_tool_events`、Router 排队状态、live browser session 等只保留内存；`conversation/state` 作为持久化事实来源。
- 调整 `SessionManager` 为“内存缓存 + 持久化后端”模型，支持懒加载恢复、写通式保存和后续替换 PostgreSQL 实现。
- 为线程级外部消息幂等恢复预留去重记录，避免重启后由于上游重投导致重复 turn 或重复回复。

## Capabilities

### New Capabilities
- `thread-context-persistence`: 定义线程上下文的持久化存储抽象、线程快照落盘模型、懒加载恢复流程以及重启后的线程状态恢复行为。

### Modified Capabilities
- `thread-managed-toolsets`: 将工具集恢复要求明确为基于线程持久化快照恢复，并要求重启后继续保持线程隔离和已加载状态的一致性。

## Impact

- Affected code: `src/session.rs`, `src/thread.rs`, `src/router.rs`, `src/main.rs`, 以及新增的 store 模块与配置入口。
- Affected runtime behavior: 线程历史、compact 相关线程状态、loaded toolsets、tool audit records 将可在重启后恢复。
- Affected APIs: `SessionManager` 将依赖统一 store trait；首版增加 SQLite 实现，后续可新增 PostgreSQL 实现而不改上层调用。
- Data impact: 需要新增线程持久化数据库文件、schema 初始化与版本迁移机制。
