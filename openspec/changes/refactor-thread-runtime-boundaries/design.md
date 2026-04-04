## Context

当前主链路虽然已经有 `Thread` 和 `ThreadRuntimeContext`，但核心边界仍然偏重：

- `Thread` 持久化消息被拆成 `system_prefix`、`feature_prefix`、`conversation` 多层；
- `AgentLoop` 仍然持有 bootstrap / init ownership；
- compact 仍然依赖 turn 与 compaction plan，而不是直接面向消息序列。

这三个点叠在一起，直接导致线程初始化、消息拼装和 compact 都不够直观。用户这轮明确要求先把架构理顺，不追加复杂能力，所以这次只收最小模型。

## Goals / Non-Goals

**Goals**

- 让 `Thread` 的持久化消息域只表现为单一消息序列。
- 稳定 system messages 在初始化时一次性写入 `Thread` 并持久化。
- `AgentLoop` 不再负责 bootstrap / init，只消费已初始化的 `Thread`。
- `ThreadRuntimeContext` 只保留 request-time live working set。
- compact 首版直接压缩全部非 `System` message，不再依赖 turn slice。

**Non-Goals**

- 这次不追求把所有 legacy `ConversationThread` / `ConversationTurn` 一次性删空。
- 这次不引入 `compact_idx`、prefix range 或其他边界索引。
- 这次不重做 memory/provider 能力，只保证它仍然是 request-time 注入。
- 这次不处理后续 feature prompt 动态重建策略，先让初始化路径跑通。

## Decisions

### 1. `Thread` 只持久化单一消息域 + 非消息状态

`Thread` 继续保持聚合根形态：

- `locator`
- `thread`
- `state`

但 `thread` 内部不再拆成 `system_prefix` / `feature_prefix` / `conversation`。它只保留一份稳定的持久化消息序列，以及必要的时间戳/元数据。

也就是说：

- 稳定 system messages 是普通持久化 `ChatMessage`
- feature 在初始化时产出的 system prompt 也是普通持久化 `ChatMessage`
- 后续普通 user / assistant / tool message 也落在同一消息序列里

这样 compact、消息导出和线程恢复都只面对一个消息域。

### 2. 线程初始化前移到 loop 之外

线程初始化不再属于 `AgentLoop`。

新的初始化边界是：

1. worker/router/session resolve 出目标 `Thread`
2. 在进入 live loop 前调用统一 `init_thread()`
3. `init_thread()` 负责：
   - 判断线程是否尚未初始化
   - 组装稳定 system messages（基础 system prompt + feature 生成的稳定 system prompt）
   - 把这些消息直接写入 `Thread`
4. 如果线程发生变化，立刻同步回 session
5. `AgentLoop` 只接收已经初始化好的 `Thread + current user input`

这样线程初始化就不再倒挂在 loop 内部。

### 3. `ThreadRuntimeContext` 只管理当前轮 live working set

`ThreadRuntimeContext` 保留，但职责收窄为：

- 包裹一个持久化 `Thread`
- 维护本轮 live `system`
- 维护本轮 live `memory`
- 维护本轮 live `chat`

导出边界：

- `Thread.messages()`：线程全部持久化消息
- `ThreadRuntimeContext.messages()`：持久化消息 + 本轮 live working set

它不负责初始化线程，也不负责维护持久化 feature prefix。

### 4. compact 只面对消息序列

compact 首版不再从 turn plan 构造 source slice。

新契约：

- compact 输入是“当前 working set 中全部非 `System` message”
- compact 输出是一个固定 replacement：
  - compacted assistant
  - `继续`
- compact 写回时保留线程中已有的持久化 `System` message，并用 replacement 覆盖原来的非 system 持久化消息

这版先不引入：

- source turn ids
- contiguous turn slice
- compact idx
- prefix range

先把 compact 的主边界收成“message in / message out”。

### 5. Turn 只保留为兼容概念

Turn 概念暂时不删，但降级为兼容层：

- 旧 compact 测试或 legacy API 仍可临时投影成 `ConversationThread`
- 主执行链路不再以 turn 作为输入
- 新 compact 主链路不再依赖 turn

## Risks / Trade-offs

- [初始化只做一次会让部分 feature prompt 后续变旧]  
  这是当前阶段接受的 trade-off。先把 ownership 收清，后续再决定哪些 prompt 要转成动态 request-time 注入。

- [legacy `ConversationThread` 还会存在一段时间]  
  接受。先把主链路挪开，再逐步拔兼容层。

- [compact 先按“全部非 system 消息”压缩，粒度比较粗]  
  接受。这正是这轮的目标，先保证边界简单、跑通，再做更细策略。

## Migration Plan

1. 更新 spec/tasks，固定“初始化前移 + 单一消息域 + compact on messages”的契约。
2. 将 `Thread` 的持久化消息模型收敛为单一序列，移除 prefix 分层。
3. 将 feature 的稳定 system prompt 组装改为初始化阶段直接写入 `Thread`。
4. 从 `AgentLoop` 移除 bootstrap/init，改由 worker 在进入 loop 前完成并同步 session。
5. 将 compact 改为 message-only 主链路，并把 legacy turn/strategy 留在兼容层。

## Open Questions

- 后续如果某些 feature prompt 需要动态更新，是继续走 request-time live system，还是允许重建持久化 system messages。
- `ConversationThread` 兼容层下一步是彻底删除，还是先只保留测试辅助投影。
