## Context

当前主链路已经把 router 的 message 上下文操控移出去了，但 loop 仍然负责维护动态 feature prompt 的拼装。至少有下面几类 prompt 还散在不同位置：

- 基础角色设定 system prompt：由 worker 在线程初始化时注入 persisted snapshot
- toolset catalog prompt：由 `ToolRegistry` 生成字符串，再由 loop 组装进本轮 system messages
- skill catalog prompt：由 skill registry 生成字符串，再由 loop 组装进本轮 system messages
- auto-compact prompt：由 loop 基于预算现算并二次刷新
- memory messages：由 loop 直接 push 到线程 live messages

这些能力虽然语义不同，但它们对请求视图的效果相同：都会让当前线程在发起 LLM 请求前多出一组 feature 相关 prompt messages。继续把它们散在 loop 中，会让 `AgentLoop` 持续膨胀成 prompt orchestration 中心。

## Goals / Non-Goals

**Goals:**

- 让 `ThreadContext` 直接表达固定的静态 feature system prompt 槽位，而不是只保留一个泛化 live preamble。
- 用统一的 `FeaturePromptProvider` trait 描述“某个 feature 如何产出 prompt messages”。
- 让动态 feature prompt 的重建通过一个固定 rebuild 入口完成，`AgentLoop` 只负责触发重建，不再手工拼接 prompt 片段。
- 保持基础 system snapshot 与动态 feature prompt 的边界清晰：前者初始化时固化，后者按 turn / 状态变化重建。

**Non-Goals:**

- 不在这个 change 中引入 runtime provider registry、service locator 或 `[id] -> provider` 的动态注册模型。
- 不在这个 change 中改变 auto-compact 阈值、tool 可见性语义或 memory provider 的命中规则。
- 不在这个 change 中优化 prompt rebuild 的缓存或 KV cache 复用。

## Decisions

### 1. `ThreadContext` 改为固定 feature 槽位，而不是通用 provider map

系统将在线程上下文中直接声明固定的静态 feature system prompt 结构，而不是把 feature prompt 存成 `HashMap<String, Vec<ChatMessage>>`。首版固定槽位至少包括：

- `toolset_catalog`
- `skill_catalog`
- `auto_compact`

这样做的原因是：

- 这些 feature 集合在当前系统里是闭集，不是插件式开放集合。
- prompt 导出顺序需要稳定且可审计，不适合交给注册顺序决定。
- 固定字段更容易调试，也更容易在测试里直接断言。

被拒绝的方案：

- 方案 A: 使用 `[id] -> provider/messages` 的动态 map
  原因: 过度通用，顺序和去重都会回到运行时约定，增加维护复杂度。

### 2. 用 `FeaturePromptProvider` 产出消息，但 provider 本身不进入 `ThreadContext`

系统会引入统一的 `FeaturePromptProvider` trait，用来描述“如何根据当前线程状态和运行时依赖产出 feature prompt messages”。但 `ThreadContext` 里只保存 feature message 槽位数据，不保存 provider trait object。

这样做可以保持边界：

- `ThreadContext` 只承载线程事实和 live message 结果
- provider 的行为逻辑留在 agent/feature 侧
- `thread.rs` 不需要反向依赖 tool registry、skill registry、compact runtime 或 memory 子系统

被拒绝的方案：

- 方案 B: 把 provider trait object 直接挂到 `ThreadContext`
  原因: 线程模型会反向依赖 runtime 行为对象，造成模块耦合升级。

### 3. 基础 system prompt 继续由初始化 snapshot 管理，动态 feature 统一 rebuild

基础角色设定 system prompt 继续只在线程初始化时进入 persisted snapshot。dynamic feature prompt 不追加到历史里，而是在每次发起请求前或 feature 状态变化后统一重建 `features_system_prompt` 与其他瞬时 live messages。

对应策略：

- `base system prompt` 属于 persisted snapshot
- `toolset/skill/auto_compact` 的稳定说明属于 `features_system_prompt`
- `memory` 属于 request-time `live_memory_messages`
- auto-compact 的动态容量属于 request-time `live_system_messages`
- feature 从关到开时重建 `features_system_prompt`；预算变化时只刷新动态容量消息

被拒绝的方案：

- 方案 C: feature 开关变化时向历史 append 一条 system message
  原因: 容易形成重复、过期 prompt，也会让状态表达依赖历史残留消息。

### 4. `auto_compact` 的稳定说明与动态预算信息必须分层

`auto_compact` 虽然是一个 feature，但它携带两类不同稳定性的 prompt 信息：

- 稳定说明：例如“auto-compact 已开启，可以在必要时调用 compact”
- 动态预算信息：例如当前上下文容量、阈值和是否接近上限

系统将把这两部分显式分层：

- 稳定说明属于 auto-compact 的稳定 feature prompt
- 动态预算信息属于可频繁刷新的 live message

这样做的原因是，预算数值会在同一轮或相邻轮中频繁变化。如果每次预算刷新都改写 auto-compact 的 system prompt，会破坏稳定前缀并放大后续缓存失效范围。首版即使不专门优化 KV cache，也应保持这个边界。

被拒绝的方案：

- 方案 D: 将 auto-compact 的稳定说明和预算数值合并成同一条 system prompt
  原因: 预算变化会导致整条 system prompt 频繁变化，破坏稳定前缀。

### 5. `AgentLoop` 只触发 rebuild，不再维护 feature prompt 拼装细节

`AgentLoop` 继续负责执行时机判断，例如：

- 何时需要在 generate 前刷新 feature prompt
- auto-compact 的预算是否变化，需要重新生成 prompt

但 loop 不再自己把 `toolset catalog + skill catalog + auto_compact prompt` 拼成临时向量。它只会：

1. 检查当前线程 feature state
2. 调用统一 rebuild 入口更新 `ThreadContext.features_system_prompt`
3. 通过 `ThreadContext.messages()` 导出完整请求消息

被拒绝的方案：

- 方案 E: 保留现在的 `build_turn_system_messages`，只把其中一部分提成 helper
  原因: 仍然会把 prompt orchestration 留在 loop 文件里，无法真正收口。

## Risks / Trade-offs

- [Risk] 固定 feature 槽位会让未来新增 prompt feature 需要修改 `ThreadContext` 结构
  Mitigation: 当前 feature 集是闭集，显式扩字段比提前设计过度通用容器更稳。

- [Risk] auto-compact 预算信息仍然依赖先计算、再刷新，重建入口可能出现两次调用
  Mitigation: 在设计中明确允许少量重算，但要求预算刷新只更新 `live_system_messages`，不改稳定 system prompt。

- [Risk] memory provider 未来如果需要更多状态，可能再次诱导把 provider registry 放进 runtime
  Mitigation: spec 明确要求 provider 只产出消息，线程只持有结果槽位，不引入 runtime registry。

## Migration Plan

1. 在 `ThreadContext` 中引入固定的 `features_system_prompt` 槽位，并调整 `messages()` 导出顺序。
2. 新增 `FeaturePromptProvider` trait，并为 toolset catalog、skill catalog、auto-compact、memory 提供固定实现。
3. 提供统一的 `features_system_prompt` rebuild 入口，并让 `AutoCompactor` 负责动态容量注入。
4. 移除 loop 中分散的 feature prompt 拼装 helper，更新测试和文档。

## Open Questions

- memory feature 的首版上下文对象是否只需要当前线程与 incoming，还是要预留更多检索参数给未来 memory provider。
