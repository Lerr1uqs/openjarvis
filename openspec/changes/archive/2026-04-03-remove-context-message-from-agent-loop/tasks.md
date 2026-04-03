## 1. 线程级 Request Context 模型

- [x] 1.1 在 `ThreadContext` 及其持久化快照中新增线程级 request context，首版固化 system prompt snapshot
- [x] 1.2 为线程创建、恢复和兼容迁移路径补齐 request context 的默认初始化与回填逻辑

## 2. 主链路迁移

- [x] 2.1 调整 `AgentWorker` 与 `AgentLoop`，改为只传当前轮 user input，由 loop 基于 `ThreadContext` 组装请求
- [x] 2.2 移除 worker / loop 热路径中的 `build_context`、`current_user_message_from_context` 和反向 backfill 依赖
- [x] 2.3 保证线程级 request context 不进入 stored turn，也不进入 compact source chat history
- [x] 2.4 继续收敛主入口，改为由 `AgentLoop::run_v1` 直接消费 `event sender + incoming + ThreadContext`，并移除 `AgentRequest` 中冗余的 `thread/history/loaded_toolsets`
- [x] 2.5 移除 loop 内部的最终消息拼装 helper，改为由 `ThreadContext.messages()` 统一导出 LLM-facing messages
- [x] 2.6 收敛 `ThreadContext` live message API，改为零参 `messages()` 和统一的 `push_message(...)`
- [x] 2.7 将 active memory 注入点固定在 AgentLoop，并把 `MessageContext` / `ContextMessage` 标记为 deprecated 兼容路径

## 3. 验证

- [x] 3.1 更新 agent loop / worker 相关 UT，覆盖已有线程仍能保留 system prompt snapshot 的行为
- [x] 3.2 新增 UT，覆盖 request-time memory 仍为动态注入，且不会被持久化或被 compact
- [x] 3.3 更新 router 相关测试，覆盖基于 `ThreadContext` 读取历史和 active thread 的兼容行为
- [x] 3.4 新增 thread UT，覆盖 `ThreadContext.messages()` 的导出顺序
