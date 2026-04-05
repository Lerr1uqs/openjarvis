## Why

当前线程上下文的容量压力只在 AgentLoop 内部以运行时 prompt 的形式存在，用户和开发者无法通过显式命令查看“这个 thread 现在吃掉了多少上下文”。当某个线程开始接近窗口上限时，只能依赖模型侧提示或日志排查，缺少一个面向线程调试的直接入口。

现在需要补一个只读线程命令，让用户可以直接查看当前 thread 的上下文占比，以及进一步查看每条 message 的估算占用，便于判断是否需要 `compact`、哪些消息最重、以及当前上下文压力是否符合预期。

## What Changes

- 新增线程级只读命令 `/context`，返回当前 thread 的上下文估算摘要，包括总占用、窗口大小与主要 token bucket 拆分。
- 新增 `/context role` 模式，逐条列出当前线程中每个 persisted message 的角色、估算 token 与占窗口比例，便于定位重消息。
- 复用现有 deterministic `chars_div4` 估算逻辑和 `llm.context_window_tokens` 配置，不引入第二套上下文估算口径。
- 命令实现保持只读，不修改线程消息、feature 状态、tool 状态或审批状态；非法参数返回明确 usage 错误。
- 补齐命令与 router UT，覆盖摘要查询、逐条明细查询、非法参数和“不触发 agent dispatch”的链路行为。

## Capabilities

### New Capabilities
- `thread-context-inspection-command`: 在线程命令层暴露当前 thread 上下文占比摘要与逐条 message 占比视图。

### Modified Capabilities

## Impact

- Affected code: `src/command.rs`、`src/compact/budget.rs` 以及对应测试 `tests/command.rs`、`tests/router.rs`。
- Runtime impact: 新增一个只读线程命令，但不引入新持久化结构，也不修改线程状态模型。
- API impact: 新增用户可见线程命令 `/context` 与 `/context role`。
- Verification impact: 需要新增命令单测与 router 链路测试，覆盖输出内容、参数校验和 agent 绕过行为。
