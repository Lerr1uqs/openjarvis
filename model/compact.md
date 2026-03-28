# Compact

## 定位

- `compact` 是线程级上下文管理器。
- 它的目标不是归档历史，而是把当前任务继续执行所需的最小 chat 上下文重新写回线程。

## 边界

- 只压缩 `chat`，不压 `system` 和 `memory`。
- 压缩结果写回 thread 历史，不写进 memory 子系统。
- 它本质上是 runtime 能力；`compact` 工具只是对这项能力的一个暴露入口。

## 关键概念

- `ContextBudgetReport`
  当前完整请求的容量快照。
- `CompactManager`
  压缩编排器。
- `CompactStrategy`
  决定压哪些 turn。
- `CompactProvider`
  负责根据旧 chat 生成结构化压缩结果。
- `AutoCompact`
  在模型侧持续暴露预算提示和 `compact` 工具的增强模式。

## 核心能力

- 在请求前估算上下文占用。
- 超过 runtime 阈值时自动压缩当前线程 chat。
- `auto_compact` 开启时，始终给模型注入预算提示并暴露 `compact`。
- 把旧 chat 替换成一个 compacted turn，而不是额外堆一份旁路摘要。

## 使用方式

- 主调用方是 `AgentLoop`，不是 Router。
- 线程级开关属于 `ThreadContext.state.features`，不是全局工具注册表状态。
