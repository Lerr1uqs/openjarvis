## Why

当前仓库里的 `memory` 仍然是空壳，但线程初始化、toolset 管理和本地文件工具链已经足够支撑首版 memory feature。继续保留“未来再说”的状态，会让 agent 无法沉淀用户明确要求保留的长期信息，也无法用渐进式披露的方式稳定暴露这些记忆。

这次变更需要先把 memory 收敛成一个可实现、边界清晰的 feature：放弃旧设想里的 request-time 主动注入正文，改为在新线程初始化时只注入 active memory 关键词词表，再由模型按需通过 memory 工具读取、搜索和列出本地记忆内容。

## What Changes

- 新增本地 memory feature，当前阶段使用工作区下 `./.openjarvis/memory/active/**/*.md` 与 `./.openjarvis/memory/passive/**/*.md` 作为默认持久化布局。
- 定义统一的 memory 文档格式，使用 frontmatter 保存 `title`、`created_at`、`updated_at`；`active` 文档额外保存 `keywords`。
- 将 `Active Memory` 定义为“线程初始化时注入的关键词词表”，而不是命中后自动把正文或摘要注入模型。
- 明确舍弃旧的“request-time 主动注入 memory 正文”方案，改为渐进式披露：模型先看到 active memory 词表，再按需调用 memory 工具读取详情。
- 新增 `memory` toolset，首版包含 `memory_get`、`memory_search`、`memory_write`、`memory_list` 四个工具。
- 明确 `memory_write`、`memory_get` 等工具的相对路径契约与 active/passive 类型规则，避免绝对路径和跨目录写入。
- 规定 active memory 写入不会热更新当前线程的已持久化 system prompt，只会在后续新线程初始化、清空上下文后的重新初始化或重启后的重新加载中生效。

## Capabilities

### New Capabilities

- `memory-feature`: 本地 active/passive memory 文档、active memory 关键词词表注入，以及 `memory` toolset 的工具契约与读取行为。

### Modified Capabilities

- `thread-context-runtime`: memory 从 request-time live message 动态注入调整为 thread init 的 active memory catalog 注入；请求期不再自动向模型注入 memory 正文。

## Impact

- Affected code: `src/agent/feature/**`、`src/agent/tool/**`、`src/agent/worker.rs`、可能新增 `src/memory/**` 或同等模块，以及对应测试和文档。
- Affected runtime data: 本地工作区会新增 `./.openjarvis/memory/active/**` 与 `./.openjarvis/memory/passive/**` 文档树。
- API impact: agent 新增 `memory` toolset 及四个 memory 工具；tool 参数需要显式区分 `active/passive` 目录语义。
- Spec impact: 需要修改现有 `thread-context-runtime` 中关于 memory 动态注入的要求，并新增独立 `memory-feature` capability。
