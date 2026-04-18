## Why

当前 subagent 生命周期有两个核心错位：

1. `spawn_subagent` 只做“准备线程”，真正执行首个任务却落在 `send_subagent`，这和 `yolo` 单次执行、`persist` 后续交互的心智模型不一致。
2. `/new` 只会重置当前 parent thread，自身挂载的 `persist` subagent 不会一起 reset/reinit，导致 parent 进入新会话后，child thread 仍保留旧上下文，线程真相分裂。

这两个问题都会直接影响主线程对 subagent 的正确使用，因此需要马上把行为契约和实现一起收敛。

## What Changes

- **BREAKING** 调整 `spawn_subagent` 语义：调用时必须同时携带首个 task，工具会负责创建或复用 child thread、执行该 task，并直接返回结果。
- **BREAKING** 调整 `send_subagent` 语义：只允许对已存在的 `persist` subagent 做后续交互，不再承担首轮启动职责。
- **BREAKING** 调整 `close_subagent` 语义：只允许关闭 `persist` subagent；`yolo` 模式不再依赖 `send/close`。
- 为 parent `/new` 增加级联语义：当前 parent thread 重初始化时，会把其名下所有 `persist` subagent 一起 reset/reinit。
- 补充对应的线程运行时日志、工具元数据与单元测试，确保主线程、persist child、yolo child 的行为边界一致。

## Capabilities

### New Capabilities
- `subagent-spawn-mode-lifecycle`: 约束 `spawn/send/close/list` 在 `persist` 与 `yolo` 两种模式下的职责边界和返回语义。
- `parent-thread-reinitialize-cascade`: 约束 parent `/new` 在重初始化当前 thread 时，同时级联重置其名下 `persist` child thread。

### Modified Capabilities
- 无

## Impact

- 受影响代码：
  - `src/agent/tool/subagent.rs`
  - `src/agent/feature/init/subagent.rs`
  - `src/agent/subagent.rs`
  - `src/thread.rs`
  - `src/command.rs`
  - `src/session.rs`
- 受影响测试：
  - `tests/agent/tool/subagent.rs`
  - `tests/agent/worker.rs`
  - `tests/command.rs`
  - 可能涉及 `tests/feature_runtime.rs` 与 `tests/router.rs`
- 对外工具接口有破坏性变化：
  - `spawn_subagent` 参数与行为变化
  - `send_subagent` 不再接受 `yolo` 首轮执行
