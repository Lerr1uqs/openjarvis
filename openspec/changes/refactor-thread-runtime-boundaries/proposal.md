## Why

当前线程主链路的复杂度主要来自三件事混在一起：

- 稳定 system messages、feature system prompt 和普通聊天历史被拆成多层持久化结构；
- 线程初始化规则放在 `AgentLoop.bootstrap_thread()` 里，导致 loop 间接成为线程初始化 ownership；
- compact 依赖 turn / strategy / replacement turn 这一整套结构，而不是直接面对消息序列。

这让 `Thread`、`AgentLoop`、compact、worker/router 的边界始终不稳定。继续在这套结构上叠功能只会不断引入额外层次，所以这次 change 要先把主模型收简单，再谈后续能力。

## What Changes

- **BREAKING** 将 `Thread` 的持久化消息域收敛为单一消息序列，不再为 `system_prefix` / `feature_prefix` 建立单独持久化分层。
- 在线程初始化阶段一次性把稳定 system messages 注入到 `Thread` 并持久化；后续 loop 只消费已初始化线程，不再拥有 bootstrap ownership。
- 保留 `ThreadRuntimeContext` 作为 request-time working set，但它只负责当前轮 live `system` / `memory` / `chat` 消息，不再承担线程初始化。
- compact 主链路改成直接处理消息序列：压缩全部非 `System` message，不再以 turn slice / strategy 作为首版主边界。
- `ConversationThread` / `ConversationTurn` 限制为 legacy 兼容层，不再作为新的主执行与 compact 主边界。

## Capabilities

### Modified Capabilities

- `thread-context-runtime`: 线程运行时契约改为“单一持久化消息域的 `Thread` + request-time `ThreadRuntimeContext` working set”。
- `agent-runtime-boundaries`: 线程初始化从 `AgentLoop` 前移到 worker/session 边界，loop 只负责消费线程与当前输入。

## Impact

- Affected code: `src/thread.rs`、`src/session.rs`、`src/session/store/**`、`src/agent/worker.rs`、`src/agent/agent_loop.rs`、`src/agent/feature/mod.rs`、`src/compact/**`、`src/router.rs` 及对应测试。
- API impact: `Thread.messages()` 将表示线程全部持久化消息；`ThreadRuntimeContext.messages()` 表示本轮完整 working set；`AgentLoop.bootstrap_thread()` 会退场。
- Storage impact: 线程持久化消息将不再分成 prefix / conversation 多层结构，compact 结果直接回写单一消息序列。
