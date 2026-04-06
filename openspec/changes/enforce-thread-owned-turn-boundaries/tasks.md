## 1. Thread Turn 模型与持久化边界

- [x] 1.1 在 `src/thread.rs` 中引入 thread-owned current turn state / turn finalization 模型，明确稳定前缀、已持久化历史和当前 turn working set 的 ownership，并保证 system messages 始终位于消息序列开头前缀。
- [x] 1.2 为 `Thread` 增加 turn 期消息写入、请求视图导出、失败/成功 finalization 和 compact 回写 API，移除对 loop 外部消息集合的依赖。
- [x] 1.3 调整 `src/session.rs` 与 `src/session/store/**` 的 thread snapshot 保存路径，使 dedup record 与 finalized turn snapshot 绑定到同一提交边界。

## 2. AgentLoop 与 Worker 重构

- [x] 2.1 重写 `src/agent/agent_loop.rs`，删除 `request_system_messages`、`live_chat_messages`、`commit_messages` 风格的局部消息真相，改为只通过 `Thread` 读写当前 turn。
- [x] 2.2 调整 `AgentLoopOutput`、worker 事件载荷和失败路径，使 loop/worker 返回 turn 级 event batch 与对应 thread snapshot，而不是增量提交消息。
- [x] 2.3 重构 `init_thread()` 与 feature prompt 路径，确保全部 system/feature prompt 只在初始化时注入 thread 开头前缀，并移除 loop 内 `request_system_messages` 及其替代容器。

## 3. Router / Session / Compact / Dispatch 接线

- [x] 3.1 重写 `src/router.rs` 的成功与失败处理逻辑，使 Router 只发送 loop 产出的 turn 级 event batch，并保存对应 thread snapshot，不再组装 user / assistant / tool / error 消息。
- [x] 3.2 将 turn 同步派发接入 channel 主链路，保证文本输出、tool 事件和 compact 事件只在 turn finalization 后按 batch 对外发送。
- [x] 3.3 调整 `src/compact/**` 与 auto-compact 逻辑，使 compact 直接读写 thread-owned active non-system view，并移除对 transient 容量 prompt 的依赖。

## 4. 验证与回归

- [x] 4.1 更新 `tests/thread.rs`、`tests/session.rs` 和 store 相关测试，覆盖 turn-owned working set、snapshot 持久化和 dedup 绑定行为。
- [x] 4.2 更新 `tests/agent/agent_loop.rs`、`tests/agent/worker.rs`，覆盖“仅由 Thread 管消息”“turn finalization 才派发/持久化”的行为。
- [x] 4.3 更新 `tests/router.rs` 与 compact 相关测试，覆盖 Router 不再组装消息、failed turn 一致性、以及 compact 基于 thread-owned active view 的行为。
