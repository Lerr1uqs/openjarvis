## Why

当前 compact 的实际执行入口被固化在 `AgentLoop` 内部，`CompactManager` 和相关编排逻辑由 loop 长期持有，导致 compact 很难作为独立 API 从外部初始化并串联调用。现在需要把 compact 提炼成独立 component，让调用方可以先压缩 thread message，再继续把压缩后的线程消息交给大模型。

## What Changes

- 新增一个可独立实例化、可独立调用的 `context-compactor` 组件，用于对传入线程消息执行 compact 并返回标准结果。
- 将 compact 的实际执行、摘要 provider 调用和 replacement thread 生成逻辑收口到该组件，而不是继续由 `AgentLoop` 自己长期持有 `CompactManager`。
- `AgentLoop` 中的 runtime compact 和模型主动触发的 `compact` tool 路径，都改为显式调用外部 compactor component 串联执行。
- 保持现有 compact 摘要格式、默认策略、replacement turn 行为和事件语义不变；本 change 不改变 compact 阈值和 budget 规则。

## Capabilities

### New Capabilities

- `context-compactor`: 提供可独立实例化、独立调用的 compact 组件接口，用于对传入线程消息执行压缩并返回标准 compact outcome。

### Modified Capabilities

- `chat-compact`: 将 runtime compact 和模型触发 compact 的执行入口改为通过外部 compactor component 串联调用，而不是由 `AgentLoop` 长期持有 compact manager。

## Impact

- Affected code: `src/compact/**`、`src/agent/agent_loop.rs` 以及对应测试。
- API impact: compact 模块会新增一个外部可调用的 component API；AgentLoop 的内部 compact 执行路径会改为委托该 API。
- Runtime impact: compact 行为保持一致，但调用方式从“loop 内部成员方法”变为“显式 component 串联调用”。
