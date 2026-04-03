# Thread

## 定位

- `ThreadContext` 是单条线程的统一事务宿主。
- 它同时承载可持久化历史和线程级运行时状态，是 Agent、Command、Session 共享的核心对象。

## 边界

- 负责保存线程身份、对话历史、工具状态、feature override、审批状态。
- 不负责消息路由，不负责调用 LLM，不负责工具注册表全局目录。

## 关键概念

- `ThreadContextLocator`
  当前线程的稳定定位信息。
- `thread_key`
  线程身份的归一化原串，格式固定为 `user_id:channel:external_thread_id`。
  它不是单独存一份业务对象，而是由 session 解析阶段基于外部消息现场即时推导出来。
- `ThreadConversation`
  线程的持久化历史，包含 `turns` 和 `tool_events`。
- `ConversationTurn`
  一轮处理后的消息集合。
- `ThreadState`
  线程级持久状态，当前拆成 `features / request_context / tools / approval`。
- `ThreadFeaturesSystemPrompt`
  当前请求前临时重建的静态 feature system prompt 槽位，固定包含 `toolset_catalog / skill_catalog / auto_compact`。
- `live_system_messages`
  当前请求的瞬时 runtime system messages，例如 auto-compact 的动态容量提示。
- `live_memory_messages`
  当前请求的瞬时 memory messages，只参与本轮 request，不进入固定 system prompt 槽位。
- `ThreadToolEvent`
  工具加载、卸载、执行的结构化审计事件。

## 核心能力

- 通过 `channel + user_id + external_thread_id` 生成稳定 `thread_key`，再派生 internal thread id。
- 以 turn 为单位保存聊天历史。
- 在线程初始化时固化 system prompt snapshot。
- 通过零参 `messages()` 对外导出完整请求消息序列，固定顺序为 persisted snapshot -> features_system_prompt -> live_system_messages -> live_memory_messages -> persisted history -> live_chat。
- 用固定的 `features_system_prompt` 槽位表达静态 feature system prompt，不在 loop 中临时拼 `Vec<ChatMessage>`。
- 通过统一 rebuild 入口整体替换 `features_system_prompt`，不把 feature 状态变化写成一次性历史消息。
- 通过 `AutoCompactor::notify_capacity(...)` 注入动态容量提示，这类 prompt 不占用固定 slot。
- 通过 `push_message(...)` 接收本轮 live chat；兼容路径下的 system / memory 会先进入 request-time 输入，再重建成 live system / live memory messages。
- 记录线程当前已加载 toolset。
- 保存 compact / auto-compact 的线程级覆盖状态。
- 在 turn 落盘前把 `pending_tool_events` 绑定到本轮 turn。
- 支持清空线程到初始状态，但保留线程身份。

## thread_key 来源

- 输入来源是外部消息里的三元组：`channel`、`user_id`、`external_thread_id`。
- `external_thread_id` 由上游平台提供；如果上游没有提供，就先归一成 `default`。
- Session 层先基于 `IncomingMessage` 生成 `SessionKey(channel + user_id)`。
- 然后再用 `SessionKey::thread_key(external_thread_id)` 拼出：
  `user_id:channel:external_thread_id`
- 这个字符串就是 `thread_key`，随后再通过 `derive_internal_thread_id(thread_key)` 稳定派生出内部 `thread_id`。

所以线程身份链路是：

`IncomingMessage -> external_thread_id -> thread_key -> internal thread_id -> ThreadContextLocator`

## 使用方式

- AgentLoop 在整个单轮执行期间直接读写 `ThreadContext`。
- Router 只负责把 `incoming` 和目标 `ThreadContext` 交给 AgentLoop，不直接操控 messages。
- toolset / skill / auto-compact 的稳定 system prompt 通过固定 `FeaturePromptProvider` 重建后写入 `features_system_prompt`。
- request-time memory 通过 memory provider 写入 `live_memory_messages`。
- auto-compact 的动态容量信息通过 `AutoCompactor` 写入 `live_system_messages`。
- 基础 system prompt 继续只在线程初始化时写入 persisted snapshot；后续 rebuild 不会覆盖它。
- Command 修改线程开关或清空历史时，也直接修改 `ThreadContext`。
- Session 持久化的对象不是零散状态，而是完整 `ThreadContext` 快照。
