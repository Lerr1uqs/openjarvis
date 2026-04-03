## Why

当前线程请求里的各类 feature prompt 分散在 `AgentLoop`、`ToolRegistry`、skill registry 和 worker 初始化路径里分别管理，导致 loop 内部持续承担 prompt 拼装和条件分支判断。现在需要把这些“会向线程请求注入 prompt 的 feature”收口到统一模型里，让 `ThreadContext` 自己表达固定的静态 feature system prompt 槽位，并让 loop 只负责触发重建而不是手工拼接。

## What Changes

- 将 `ThreadContext` 的 prompt 视图明确拆成六层：persisted snapshot、persisted feature state、features_system_prompt、live_system_messages、live_memory_messages、live chat messages。
- 为动态 feature prompt 引入统一的 `FeaturePromptProvider` trait，用于产出 toolset catalog、skill catalog、auto-compact prompt 和 memory messages。
- `ThreadContext` 新增固定的 `features_system_prompt` 槽位，而不是继续在 loop 中以临时 `Vec<ChatMessage>` 手工拼装各类静态 feature prompt。
- `AgentLoop` 改为在合适时机重建 `features_system_prompt`，然后直接使用 `ThreadContext.messages()` 导出完整请求消息。
- 基础角色设定 system prompt 继续由线程初始化阶段写入 persisted snapshot；动态 feature 的开关变化通过重建 `features_system_prompt` 生效，而不是追加历史消息。
- `auto_compact` 的稳定功能说明和实时上下文容量信息显式分层；预算变化时只通过 `AutoCompactor` 更新 `live_system_messages`，而不重写稳定 system prompt。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `thread-context-runtime`: 线程运行时 prompt 模型改为固定 `features_system_prompt` + `FeaturePromptProvider`/`AutoCompactor` 驱动，AgentLoop 不再手工拼装分散的 feature prompt。

## Impact

- Affected code: `src/thread.rs`、`src/agent/agent_loop.rs`、`src/agent/worker.rs`、`src/agent/tool/**`、可能新增 `src/agent/feature/**`，以及对应测试和模型文档。
- API impact: 线程 live prompt 相关接口会从“通用 push + loop 内拼装”调整为“固定 feature 槽位 + rebuild 入口”；会新增 `FeaturePromptProvider` trait。
- Runtime impact: auto-compact、toolset catalog、skill catalog 和 memory 的 prompt 注入语义保持不变，但注入入口和管理边界会收口到线程模型。
