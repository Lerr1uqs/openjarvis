## 1. 运行时与数据结构

- [x] 1.1 删除 `Thread` / `Session` / `Worker` / `Router` / store 中的全部 `turn_id` 字段和访问接口。
- [x] 1.2 删除 `pending_tool_events` 及其 buffer 逻辑，要求 tool audit 只能在 active turn 内记录。
- [x] 1.3 让 active turn 变成纯本地执行期状态，不再进入持久化 thread snapshot。

## 2. 持久化与消息链路

- [x] 2.1 重构 `push_message(...)` 的持久化路径，确保写入 store 时不携带 local turn state。
- [x] 2.2 重构 external message dedup，移除 `turn_id` 关联字段和 sqlite schema 对应列。
- [x] 2.3 清理 worker/router 中对 committed event 的 turn 级日志和 payload 依赖。

## 3. 回归测试

- [x] 3.1 改写 `tests/thread.rs`、`tests/session.rs`，覆盖“无 turn_id”“无 pending buffer”“turn 不持久化”。
- [x] 3.2 改写 `tests/agent/worker.rs`、`tests/router.rs`、`tests/agent/agent_loop.rs`，覆盖 message 级事件发送且无 turn identity。
- [x] 3.3 改写 sqlite/memory store 相关测试，覆盖 dedup 去掉 `turn_id` 后的 roundtrip。
