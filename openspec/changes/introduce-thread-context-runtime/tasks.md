## 1. ThreadContext 数据模型

- [x] 1.1 新增 `ThreadContext`、`ThreadConversation`、`ThreadState` 及其子结构，明确 conversation / feature / tool / approval 分层
- [x] 1.2 调整 Session 层的线程加载与存储接口，使其能够围绕 `ThreadContext` 读写线程数据
- [x] 1.3 为现有 `ConversationThread` 与新结构补充兼容映射，避免一次性打断现有调用链
- [x] 1.4 收敛线程身份规则：将 `IncomingMessage.thread_id` 更正为 `external_thread_id`，定义 `thread_key = user:channel:external_thread_id`，并让 internal thread id 由该 key 稳定派生

## 2. 兼容层与 Deprecated 迁移

- [x] 2.1 为现有 thread-scoped runtime API 添加 Rust `#[deprecated]` 标记和迁移说明
- [x] 2.2 将旧的 `ToolRegistry` 线程入口转发到 `ThreadContext` 新路径，先保留兼容行为
- [x] 2.3 为旧的 compact 线程 override 入口提供兼容转发，避免命令和循环在迁移期双写分裂

## 3. 主链路迁移

- [x] 3.1 调整 `AgentWorker` 和 `AgentLoop`，让主循环直接接收和操作 `ThreadContext`
- [x] 3.2 将线程工具可见性计算、工具调用分发和 tool event 记录迁移到 `ThreadContext`
- [x] 3.3 调整 thread-scoped command 处理路径，使 `/auto-compact` 等线程命令基于目标 `ThreadContext` 读写状态
- [x] 3.4 删除 global command 分类，统一要求所有命令先解析目标线程，再在对应 `ThreadContext` 上执行

## 4. 文档与验证

- [x] 4.1 更新 `arch/system.md`，同步 `ThreadContext -> ThreadConversation -> ToolRegistry` 的新分层
- [x] 4.2 为 `ThreadContext`、deprecated 兼容层、线程命令和 AgentLoop 迁移补充 UT，覆盖线程隔离与兼容行为
- [x] 4.3 在全部调用点迁移完成后，清理已无引用的旧线程运行时入口
- [x] 4.4 更新 OpenSpec 中文文档，补充 `thread_key` / internal thread id 规范，并修正命令作用域描述
