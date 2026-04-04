## 1. 线程消息模型收敛

- [x] 1.1 将 `Thread` 的持久化消息模型收敛为单一消息序列，移除 `system_prefix` / `feature_prefix` 分层
- [x] 1.2 保留 `ThreadRuntimeContext` 作为 request-time working set，并明确 `Thread.messages()` 与 `ThreadRuntimeContext.messages()` 的边界
- [x] 1.3 调整 `SessionManager`、session store、router 和 command 主链路，使其统一围绕新的 `Thread` 快照读写

## 2. 初始化 ownership 前移

- [x] 2.1 将稳定 system messages 的生成与写入收口到统一 `init_thread()`，不再由 `AgentLoop` 持有 bootstrap ownership
- [x] 2.2 让 worker 在进入 live loop 前初始化线程，并在初始化修改线程时立刻同步 session
- [x] 2.3 保证 request-time live system/memory/chat messages 不会随线程初始化或持久化写回

## 3. compact 收敛到消息边界

- [x] 3.1 将 compact 主链路改为直接处理全部非 `System` message
- [x] 3.2 从 agent 主链路移除对 turn-based compaction plan / source slice 的依赖
- [x] 3.3 把 `ConversationThread` / `ConversationTurn` 限制为兼容层，不再作为新的主执行输入

## 4. 验证与文档

- [x] 4.1 更新 thread/session/store UT，覆盖线程初始化、单一消息域和 revision 合并行为
- [x] 4.2 更新 worker/router/agent loop/compact 测试，覆盖“初始化前移 + compact on messages”行为
- [x] 4.3 更新 `arch/system.md`、`model/thread.md`、`model/agent-loop.md` 和模块说明文档
