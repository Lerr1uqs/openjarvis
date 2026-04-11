# Memory

## 定位

- `Memory` 是工作区级、本地持久化的长期知识层。
- 它负责保存跨线程、跨重启仍然成立的用户记忆。
- 它不属于 `Thread` 聚合本身，也不属于 request-time 临时 working set。

## 职责

- 维护工作区下 `./.openjarvis/memory` 的事实来源。
- 区分 `active memory` 和 `passive memory` 两类记忆。
- 在 thread 初始化或重初始化时，为 `active memory` 派生稳定的 `keyword -> path` catalog 注入模型。
- 通过 `memory` toolset 提供 `memory_get`、`memory_search`、`memory_write`、`memory_list`。

## 严格边界

- `Memory` 不负责保存线程消息历史；线程消息仍由 `Thread` 持久化。
- `Memory` 不负责 request-time 自动召回正文；正文读取必须通过 memory 工具显式发生。
- `Memory` 不负责 embedding、向量索引、远程后端或权限模型。
- `Memory` 不负责热更新已经初始化完成的线程级 catalog；catalog 刷新只发生在 thread 初始化或重初始化。

## 关键概念

- `MemoryRepository`
  本地 memory 仓库的统一访问边界，负责扫描、解析、写入和 catalog 派生。
- `active memory`
  需要在 thread 初始化时先向模型披露“有哪些记忆”的记忆类型；(也就是注入到system prompt中) 正文不主动注入。
- `passive memory`
  不进入 thread init catalog，只能通过搜索、列表和读取工具按需使用的记忆类型。
- `memory document`
  一个 markdown 文档事实，带有 frontmatter metadata 和正文。
- `active memory catalog`
  一个稳定的 `keyword -> relative path` 映射，只作为 thread init 的 system snapshot 组成部分存在。
- `memory toolset`
  一个线程级按需加载的 toolset，负责对本地 memory 仓库做渐进式披露。

## 存储模型

- memory 事实来源固定为工作区下的 `./.openjarvis/memory`。
- 目录语义固定为：
  - `./.openjarvis/memory/active/**/*.md`
  - `./.openjarvis/memory/passive/**/*.md`
- 文档类型由目录决定，而不是由额外 `type` metadata 决定。
- 所有 memory 文档都至少包含：
  - `title`
  - `created_at`
  - `updated_at`
- `active` 文档额外要求非空 `keywords`，并在 active 仓库范围内保持全局唯一。

## 运行时接线

- Worker 在 thread 初始化时从 `MemoryRepository` 读取 active memory，并构造稳定的 catalog system prompt。
- 这个 catalog 会持久化进 `Thread` 的system prompt，和其他固定 feature prompt 一起成为线程快照的一部分。
- `AgentLoop` 在普通请求轮次中不会因为用户命中 keyword 自动追加 memory 正文或摘要（这是以前的提案，现在废弃了）。
- 模型若需要记忆详情，应先加载 `memory` toolset，再用 `memory_get` 或 `memory_search` 等工具逐步读取。

## 渐进式披露原则

- thread init 只披露“记忆存在性”和“访问入口”。
- `memory_list` / `memory_search` 只披露结构化候选，不披露正文。
- `memory_get` 才披露单篇正文。
- `memory_write` 负责把新事实写回本地仓库，但不会回写当前线程已持久化的旧 catalog。

## 验收标准

- 当 agent 调用 `memory_write` 写入 active/passive memory 后，对应 markdown 文档能在 `./.openjarvis/memory` 中被检索到。
- 当线程首次初始化或清空后重初始化时，active memory 的 `keyword -> path` catalog 会进入 thread 的稳定 system snapshot。
- 当用户消息命中 active keyword 时，系统不会自动把对应正文注入请求。
- 当模型未加载 `memory` toolset 时，看不到 `memory_get`、`memory_search`、`memory_write`、`memory_list`。
- 当模型加载 `memory` toolset 后，可以通过 `type + relative path` 稳定读取、搜索、列出和写入记忆。
