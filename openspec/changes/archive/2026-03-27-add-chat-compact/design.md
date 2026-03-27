## Context

当前会话层只有一个临时性的 `max_messages_per_thread` 裁剪策略，它按消息条数裁掉旧消息，却不感知 token，也不会生成可恢复的压缩结果。这和 `arch/system.md` 里已经提出的 compact 目标不一致：真正需要的是线程级上下文管理，而不是单纯丢弃旧消息。

本次讨论里已经确认了 compact 的几个关键边界：

- `Compact` 本身是线程级上下文管理器，不天然是 tool。
- `AutoCompact` 是构建在 `Compact` 之上的可选特性。只有开启 `AutoCompact` 才需要向模型暴露 compact tool 和上下文容量信息。
- compact 仅作用于 `chat`，不作用于 `system` 和 `memory`。
- compact 结果必须写回 message 历史，首版压成一个 compacted turn，两条 message 分别使用普通 `assistant` 和 `user` 角色。
- 首版 compact 完成后，active history 中被压缩的旧 chat 直接被替换掉；但需要为未来改成 archive / shadow copy 留出扩展点。
- `chat` 范围内天然已经包含用户消息、assistant 消息、tool call 和 tool result，因此 compact 输入实际上是完整 chat history，而不是只处理纯文本对话。

这次变更会同时影响 session/thread 持久化、上下文构造、LLM 调用前预算检测，以及 ToolRegistry 的工具可见性投影。

## Goals / Non-Goals

**Goals:**
- 提供真正的线程级 compact 机制，替代按消息数粗暴裁剪的临时方案。
- 在触发 compact 时，把当前线程全部历史 `chat` 压缩成一个 compacted turn，并继续参与后续对话与后续 compact。
- 提供 token 预算估算能力，覆盖 `system`、`memory`、`chat`、visible tools 和预留输出。
- 保持 runtime-managed compact 始终存在；在 `auto_compact` 打开时，再为模型增加主动 compact 的能力。
- 通过 `CompactStrategy` 统一管理不同压缩策略，首版只实现“compact 全量 chat”。
- 为未来保留原始消息 archive、更多 compact 策略和更丰富工具显隐条件留出扩展点。

**Non-Goals:**
- 本次不压缩 `system` 和 `memory`。
- 本次不做 rolling summary、只压一部分 turn 或保留最近 N 轮的双轨模式。
- 本次不把 compact 结果存进 memory 系统。
- 本次不实现 source message 的 archive 持久化，只在接口层保留未来支持空间。
- 本次不尝试让模型在 `auto_compact` 关闭时感知容量或调用 compact tool。

## Decisions

### 1. `CompactManager` 是线程级上下文管理入口，`AutoCompact` 只是其可选模型接口

本次将 compact 设计成独立的线程级管理器，而不是普通 tool handler。它负责：

- 估算当前线程上下文 token 占用
- 判断是否触发 runtime compact
- 调用 `CompactStrategy`
- 把压缩结果回写为新的 compacted turn
- 在 `auto_compact` 开启时，决定是否向模型暴露 compact tool 和上下文预算信息

`auto_compact` 不改变 compact 的本质，它只是给模型一个“提前 compact”的入口。即使 `auto_compact` 关闭，runtime 在硬阈值前也仍然会执行 compact。

Alternative considered:
- 把 compact 完全做成普通 tool，只在模型调用时触发。
  Rejected，因为一旦接近硬上限，模型未必还有足够余量做出正确 compact 决策；compact 必须有 runtime 兜底。

### 2. compact 结果写回为一个 compacted turn，包含两条普通 message

首版 compact 结果不是额外 metadata，也不是 memory，而是一个普通 chat turn：

- compacted `assistant` message：明确说明“这是压缩后的上下文”，并提炼任务目标、用户约束、当前背景、当前规划、已完成、未完成和关键事实
- follow-up `user` message：固定写入“继续”，让后续模型生成自然延续当前任务

这两条 message 仍然属于 `chat`，而不是 `memory`。它们会在后续请求里像正常 chat history 一样参与上下文构造，并且在下一次 compact 时继续被纳入输入。

由于 assistant 内容本身已经明确声明“这是压缩后的上下文”，首版不再为 compact 结果新增额外 message metadata 或 turn metadata。

Alternative considered:
- 只压成一条 summary message。
  Rejected，因为需要额外一个 follow-up user message 把对话重新续接回正常节奏。

Alternative considered:
- 把 compact 结果塞进 `memory`。
  Rejected，因为 compact 产物本质上是线程 chat history 的替代表达，不是长期 memory。

### 3. 首版策略是“全量 chat 替换”，但通过 `CompactionPlan` 为未来保留 source 处理扩展

V1 的 active history 处理方式是：

1. 选中当前线程全部历史 chat messages
2. 调用 compact provider 生成 compacted turn
3. 删除被 compact 的旧 chat messages
4. 插入新的 compacted turn

也就是 active history 里直接替换，不做 source archive。

为了给未来保留空间，compact 核心流程不应把“删除 source”写死，而应由 `CompactionPlan` 表达 source handling。例如：

- `DropSource`：V1 默认策略
- `ArchiveSource`：未来可扩展
- `KeepShadowCopy`：未来可扩展

这样以后要改存储策略时，不必重写 compact 主流程。

Alternative considered:
- 旧消息只附加不替换。
  Rejected，因为 active history 并不会真正缩小，compact 失去主要价值。

### 4. compact 通过 `CompactStrategy` 抽象管理，首版默认 `CompactAllChatStrategy`

本次引入 `CompactStrategy` 作为压缩策略插件抽象。其输入是线程当前 chat history 与预算信息，输出是 `CompactionPlan`。首版仅实现一种策略：

- `CompactAllChatStrategy`

它在触发时直接选择当前线程的全部历史 chat 作为压缩输入。后续如果要演进到：

- 仅压缩旧 turn
- 保留最近 N 轮
- 对大 tool result 做特殊策略

都可以在不改 manager 调度逻辑的前提下替换策略。

Alternative considered:
- 先把“全量 chat 压缩”写死在 manager 里。
  Rejected，因为这会让后续策略替换、测试注入和行为对比都变差。

### 5. 上下文预算按“最终送给模型的完整请求”估算，而不是只看 chat

compact 只作用于 chat，但预算估算必须面向完整 LLM 请求。预算报告至少应拆分为：

- `system_tokens`
- `memory_tokens`
- `chat_tokens`
- `visible_tool_tokens`
- `reserved_output_tokens`
- `total_estimated_tokens`
- `context_window_tokens`
- `utilization_ratio`

这样 runtime 可以正确判断是否触发 compact，`auto_compact` 也能把容量信息准确暴露给模型。

配置建议拆分为两层：

- `llm`: 模型协议、上下文窗口、tokenizer 标识
- `agent.compact`: compact 开关、阈值、预留输出、策略和 `auto_compact`

Alternative considered:
- 只按 chat token 数决定是否 compact。
  Rejected，因为 tool schema 和 system/memory 也会显著占用窗口，不看全量请求容易误判。

### 6. `auto_compact` 下的 compact tool 走 ToolRegistry 显隐投影，而不是始终可见

为满足“只有开启 AutoCompact 才向模型暴露 compact tool”的约束，ToolRegistry 需要新增线程运行时的工具可见性投影。某个工具即使已经注册，也不代表对当前线程当前时刻一定可见。

compact tool 的显隐规则为：

- `auto_compact = false`：compact tool 对模型不可见
- `auto_compact = true` 且预算低于软阈值：compact tool 默认不可见
- `auto_compact = true` 且预算到达软阈值但未到硬阈值：compact tool 可见，并向模型注入容量信息
- 到达硬阈值：runtime 直接 compact，tool 是否可见已不再关键

这要求 `ToolRegistry::list_for_thread(...)` 从“列出已注册工具”演进为“列出当前线程当前状态下可见工具”。

Alternative considered:
- compact tool 始终注册且始终可见。
  Rejected，因为这和 `auto_compact` 的 feature 语义不一致，也会无谓膨胀工具上下文。

### 7. compacted turn 保持普通 `assistant/user` 角色，不额外引入持久化 metadata

由于 compact 结果最终会被重写成：

- 一条声明“这是压缩后的上下文”的 assistant message
- 一条固定为“继续”的 user message

因此首版无需在 `ChatMessage` 或 `ConversationTurn` 上新增额外 metadata。对于 runtime 和调试来说，assistant message 的固定前缀已经足够表达这是 compact 产物；而再次 compact 时，也只需要把它当作普通 chat history 继续压缩即可。

## Risks / Trade-offs

- [compact prompt 质量不稳定会影响恢复效果] -> 固定 summary 输出结构，至少强制保留任务目标、用户约束、规划、已完成、未完成和关键事实。
- [直接替换 source history 会降低调试可追溯性] -> V1 接受该取舍，但通过 `CompactionPlan` 明确保留未来 archive 扩展点。
- [预算估算和真实 provider token 统计可能存在偏差] -> 使用 tokenizer 估算作为前置判断，并为未来引入 provider usage 校准留出口。
- [工具可见性从静态转为动态会增加 ToolRegistry 复杂度] -> 把“定义”和“线程可见性投影”分层，避免污染具体工具实现。
- [compact 输出仍然占用 chat 区域，反复 compact 可能累积摘要误差] -> 首版先接受这一代价，后续再用更多策略或 archive 机制改善。

## Migration Plan

1. 新增 compact 配置与预算估算模块，保留现有 `max_messages_per_thread` 作为临时兼容路径。
2. 引入 `CompactManager`、`CompactStrategy` 和 compacted turn 写回逻辑。
3. 将 session/thread 的 chat 裁剪路径替换为 compact 路径，并保留 future source handling 扩展点。
4. 扩展 ToolRegistry 的线程级工具可见性投影，为 `auto_compact` 控制 compact tool 显隐。
5. 更新 `arch/system.md` 里的 `agent context容量`、`compact`、`auto-compact` 章节。
6. 补足预算估算、runtime compact、compacted turn 写回和 auto-compact 可见性的测试。

Rollback strategy:
- 可以先关闭 compact 配置，继续沿用现有的按消息数量裁剪逻辑；由于新的 source archive 还未启用，回滚路径相对直接。

## Open Questions

- compact provider 的 prompt 是否需要专门模型配置，还是默认复用当前主模型；这会影响成本和延迟，但不阻塞当前 change 立项。
