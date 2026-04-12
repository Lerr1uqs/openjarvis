## Why

当前线程请求里的各类 feature prompt 分散在 `AgentLoop`、`ToolRegistry`、skill registry 和 worker 初始化路径里分别管理，导致 loop 内部持续承担 prompt 拼装和条件分支判断。现在需要把这些“会向线程请求注入 prompt 的 feature”收口到统一模型里，但稳定 prompt 应继续直接表现为 `Thread.messages()` 开头的 `System` 前缀，而不是额外新增 `features_system_prompt` 成员。

## What Changes

- 将 `ThreadContext` 的 prompt 视图收口为：持久化消息序列中的稳定 `System` 前缀、persisted feature state、live_system_messages、live_memory_messages、live chat messages。
- 用统一的 feature 构造入口生成 toolset catalog、skill catalog、auto-compact prompt 和 memory messages，但不再要求 `FeaturePromptProvider` trait 成为主模型。
- 稳定 feature prompt 继续直接写入 `Thread.messages()` 开头的 `System` 前缀，而不是新增固定 `features_system_prompt` 槽位。
- `AgentLoop` 改为在合适时机刷新稳定 feature prompt 相关状态，然后直接使用 `ThreadContext.messages()` 导出完整请求消息。
- 基础角色设定 system prompt 继续由线程初始化阶段直接写入持久化消息序列；动态 feature 的开关变化通过刷新状态和重新物化提示生效，而不是追加历史消息。
- `auto_compact` 的稳定功能说明和实时上下文容量信息显式分层；预算变化时只通过 `AutoCompactor` 更新 `live_system_messages`，而不重写稳定 system prompt。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `thread-context-runtime`: 线程运行时 prompt 模型改为稳定 `System` 前缀 + persisted feature state + live message 驱动，AgentLoop 不再手工拼装分散的 feature prompt。

## Impact

- Affected code: `src/thread.rs`、`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/agent/tool/**`、可能新增 `src/agent/feature/**`，以及对应测试和模型文档。
- API impact: 线程 live prompt 相关接口会从“通用 push + loop 内拼装”调整为“稳定前缀 + 统一 feature 刷新入口”；不会再引入固定 `features_system_prompt` 成员作为主模型。
- Runtime impact: auto-compact、toolset catalog、skill catalog 和 memory 的 prompt 注入语义保持不变，但注入入口和管理边界会收口到线程模型。
