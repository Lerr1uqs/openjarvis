# Compact

## 定位

- `compact` 是线程上下文压缩能力。
- 它的输入边界是 message 序列，不是 turn slice。
- 它的职责是把旧消息压缩成新的替代消息；是否写回 `Thread` 由调用方决定。

## 严格边界

- 不压缩持久化 `System` messages。
- 主链路只压缩非 `System` message。
- 不把 compact 结果写入 memory 子系统。
- `compact` 工具只是 runtime compact 能力的一个暴露入口。
- `CompactManager` 不依赖 `Turn`、`Thread`、session 或 Router。

## 关键概念

- `ContextBudgetReport`
  当前请求的上下文容量快照。
- `CompactManager`
  负责调用 provider 并构造 compact 后替代消息。
- `CompactProvider`
  负责根据旧消息生成结构化 compact summary。
- `CompactSummary`
  provider 输出的压缩摘要。
- `AutoCompactor`
  负责注入动态容量提示，不负责真正压缩历史。

## 主执行模型

- detached / offline compact
  - 输入是线程当前全部持久化非 `System` message
- runtime compact
  - 输入是 `persisted non-system messages + pending live chat messages`

compact summary 会被物化成两条消息：

1. compacted assistant message
2. user `继续`

## 调用关系

- `CompactManager::compact_messages(...)` 只接收 `Vec<ChatMessage>` 并返回 `MessageCompactionOutcome`
- `AgentLoop` 或其他调用方负责把 compact 结果写回 `Thread`
- `Thread` 写回时保留 system prefix，只替换非 system 历史
