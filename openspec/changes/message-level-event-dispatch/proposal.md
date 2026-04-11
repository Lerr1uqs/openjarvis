## Why

当前系统把用户可见事件的外发时机绑定在 finalized turn 上，导致一次输入触发的中间文本、tool 调用结果和失败信息都要等到 turn 结束后才统一发送。这个模型虽然保证了外发结果与持久化快照严格一致，但交互延迟过高，不适合需要及时反馈中间进展的 agent 场景，因此现在需要把外发语义改成按 message / event 级别发送。

## What Changes

- **BREAKING** 将 agent 对外发送语义从“finalized turn event batch”改为“单条 message / event committed 后立即发送”，不再把 tool call 与 tool result 打包成对发送。
- 保留 `Thread` 作为消息 owner，但把“对外发送边界”与“turn 最终持久化边界”解耦。
- 调整 `AgentLoop`、`Worker`、`Router` 和 `Session` 的协作方式，使文本输出、tool 事件和失败通知可以按顺序逐条发送。
- 定义失败 turn 在 message 级发送模型下的收尾规则，明确哪些结果已经外发、哪些状态只进入最终 snapshot。
- 调整 dedup / restore / 审计语义，确保 message 级外发后仍能得到可解释的最终线程状态。

## Capabilities

### New Capabilities
- `message-level-dispatch`: 定义 message / event 级即时外发、失败收尾、顺序保证与 router/session 协作边界。

### Modified Capabilities
- `thread-context-runtime`: 调整当前 turn working set 与 turn finalization 的语义，使其不再等同于唯一外发边界，而只保留消息 ownership 与最终快照职责。

## Impact

- 影响代码路径：`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/router.rs`、`src/session.rs`、`src/thread.rs` 及对应测试。
- 影响外部行为：channel 将更早收到中间文本、tool 结果与失败通知；这属于用户可见的 breaking change。
- 影响 OpenSpec：需要引入新的 dispatch capability，并修改现有 `thread-context-runtime` 对 turn / finalization / dispatch 的约束。
