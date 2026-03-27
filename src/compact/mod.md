# compact 模块总览

## 作用

`compact/` 负责线程级上下文压缩。它的目标不是把历史做归档保存，而是在不丢失当前任务关键状态的前提下，把过长的 `chat` 历史替换成更小的继续工作上下文。

## 子模块

- `budget.rs`
  容量估算层。负责估算一次完整 LLM 请求的上下文预算占用。
- `manager.rs`
  编排层。负责选择压缩计划、调用 provider、生成替代 turn。
- `provider.rs`
  压缩提供者层。负责定义 compact 请求、prompt、摘要结果，以及基于 LLM 的实现。
- `strategy.rs`
  策略层。负责决定“哪些 turn 应该被压缩”。

## 核心概念

- `ContextBudgetReport`
  当前完整请求的容量快照，描述 system、memory、chat、tool 等部分分别占了多少。
- `CompactStrategy`
  压缩选择策略，决定压缩范围。
- `CompactionPlan`
  一次可执行的压缩计划，描述具体替换哪些 turn。
- `CompactProvider`
  负责根据历史消息生成结构化压缩结果的提供者。
- `CompactSummary`
  压缩后提炼出的任务状态摘要。
- `Compacted Turn`
  替代原历史的新 turn，通常由一条压缩后的 assistant 消息和一条“继续”用户消息组成。

## 关键边界

- compact 只作用于 `chat`，不直接压 `system` 和 `memory`。
- compact 结果会写回 thread 历史，而不是写进 memory 子系统。
- compact 是线程级上下文管理器，不是天然的普通工具。
- `auto_compact` 开启后，runtime 会在每次 generate 时把当前 `ContextBudgetReport` 以提示形式暴露给模型，并让 `compact` 工具始终可见。

## 设计意图

- 核心不是“省一点 token”，而是“让后续轮次仍然能恢复任务现场”。
- 这里维护的是继续执行所需的最小工作集，而不是做长期知识沉淀。
