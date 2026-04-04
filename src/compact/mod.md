# compact 模块总览

## 作用

`compact/` 负责线程级上下文压缩。它的目标不是归档历史，而是在不丢失当前任务关键状态的前提下，把过长的消息历史替换成更小的继续工作上下文。

## 子模块

- `budget.rs`
  容量估算层。负责估算一次完整 LLM 请求的上下文预算占用。
- `manager.rs`
  编排层。负责调用 provider，并生成替代消息。
- `provider.rs`
  压缩提供者层。负责定义 compact 请求、prompt、摘要结果，以及基于 LLM 的实现。

## 核心概念

- `ContextBudgetReport`
  当前完整请求的容量快照，描述 `system`、`memory`、`chat`、`tool` 等部分分别占了多少。
- `CompactProvider`
  负责根据历史消息生成结构化压缩结果的提供者。
- `CompactSummary`
  压缩后提炼出的任务状态摘要。
- `MessageCompactionOutcome`
  一次 compact 的输入消息数、摘要和替代消息结果。

## 关键边界

- compact 只作用于非 `System` message。
- compact 结果不会写进 memory 子系统。
- `CompactManager` 只依赖 provider 和 message 输入。
- `auto_compact` 开启后，runtime 会在 generate 前把当前 `ContextBudgetReport` 以提示形式暴露给模型，并让 `compact` 工具可见。

## 设计意图

- 核心不是“省一点 token”，而是“让后续轮次仍然能恢复任务现场”。
- 这里维护的是继续执行所需的最小工作集，而不是做长期知识沉淀。
