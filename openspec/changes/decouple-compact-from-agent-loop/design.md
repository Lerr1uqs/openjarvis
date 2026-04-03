## Context

当前 compact 相关逻辑分成两层：

- `compact/` 模块已经有 `CompactManager`、provider、strategy 等基础能力。
- 但真正把一个线程送去 compact、替换 active history、清理 loop 内部状态的执行入口，仍然由 `AgentLoop` 自己长期持有并直接驱动。

这使得 compact 的使用方式被固化成“AgentLoop 的一个内部成员能力”：

- 外部调用方如果想在 loop 之外手动 compact 某个线程，需要复制 loop 里的编排逻辑。
- runtime compact 与模型主动触发 `compact` 的路径共用的是 loop 内部方法，而不是一个显式的外部 component contract。
- compact 很难被当作一个可插拔步骤插进其他调用链中，例如“先 compact，再继续发起本轮 LLM 请求”。

## Goals / Non-Goals

**Goals:**

- 提供一个可独立实例化、独立调用的 `ContextCompactor` component。
- 让 `AgentLoop` 中的 compact 执行路径通过该 component 串联调用，而不是继续长期持有 `CompactManager`。
- 保持现有 compact 结果结构、strategy/provider 组合方式和 thread replacement 语义不变。

**Non-Goals:**

- 不在这个 change 中重做 budget estimator；compact 阈值判断和 auto-compact prompt 规则可以暂时保留在 AgentLoop。
- 不在这个 change 中改变 compact summary prompt、默认 `CompactStrategy` 选择或 `compact` tool 名称。
- 不在这个 change 中引入 HTTP/RPC 形式的远程 compact 服务。

## Decisions

### 1. 引入独立的 `ContextCompactor` component

系统将新增一个独立的 `ContextCompactor` component，由调用方显式实例化，并对传入线程消息执行 compact。这个 component 负责：

- 接收待 compact 的 thread snapshot 或等价的 thread message 视图
- 调用 compact provider 生成摘要
- 应用 compact strategy 并产出标准 `CompactionOutcome`

这样外部调用方可以明确地按下面的链路工作：

1. 初始化一个 compactor
2. 对当前线程消息执行 compact
3. 使用 compact 后的线程消息继续发起 LLM 请求

被拒绝的方案：

- 方案 A: 继续只暴露 `CompactManager`，把编排逻辑留在 `AgentLoop`
  原因: 仍然无法形成面向外部调用方的稳定 component contract。

### 2. `AgentLoop` 通过显式 component 调用 compact，而不长期持有 compact manager

这个 change 里，`AgentLoop` 不再把 compact manager 作为长期成员持有。runtime compact 和模型触发 `compact` 的两条执行路径都改为显式构造或获得一个 `ContextCompactor`，并通过统一 contract 调用。

这满足两个目标：

- `AgentLoop` 不再是 compact 的唯一入口
- loop 内外都能复用同一套 compact 执行 contract

被拒绝的方案：

- 方案 B: 把 `ContextCompactor` 作为新的 `AgentLoop` 成员注入
  原因: 仍然把 compact 的使用方式固化在 loop 内部，不符合“外部可随时初始化并串联调用”的目标。

### 3. component 保持纯 compact 职责，不接管 Router / event / turn side effect

`ContextCompactor` 只负责 compact 本身的执行与结果产出，不直接发送 router event，也不直接修改 `AgentLoop` 的 `turn_messages`、`prepend_incoming_user` 等 loop 状态。调用方仍然负责：

- 何时调用 compact
- 如何在 compact 成功后覆写 thread active history
- 如何记录事件、tool result 和后续 LLM request

这样可以让 component 在 loop 内外都保持可复用，而不会携带 loop 专属副作用。

被拒绝的方案：

- 方案 C: 让 compactor 直接发事件并写入 loop 状态
  原因: 会让 component 再次和 AgentLoop / Router 耦合。

## Risks / Trade-offs

- [Risk] compact 后的线程覆写、事件记录等外围逻辑仍然在调用方，可能出现多个调用方各自复制一段后处理
  Mitigation: component 必须返回稳定且充分的 outcome，让调用方只需要做最薄的一层接线。

- [Risk] 每次需要 compact 时都显式初始化 component，可能引入轻微重复构造成本
  Mitigation: component 设计成轻量级，主要持有 `Arc` 和配置值，避免昂贵初始化。

- [Risk] runtime compact 与 tool-requested compact 的外围行为如果分叉，可能造成调用 contract 漂移
  Mitigation: spec 明确要求两条路径共用同一 compactor execution contract。

## Migration Plan

1. 在 `compact/` 模块中提炼独立的 `ContextCompactor` API。
2. 让 AgentLoop 的 runtime compact 和 tool-triggered compact 改为委托该 component。
3. 保持现有 compact provider / strategy / replacement turn 流程不变，只调整调用边界。
4. 更新测试，确保 standalone compactor 与 loop delegation 都覆盖到位。

## Open Questions

- `ContextCompactor` 的首版输入输出是否直接围绕 `ConversationThread`，还是同时提供面向 `ThreadContext` 的便捷入口。
