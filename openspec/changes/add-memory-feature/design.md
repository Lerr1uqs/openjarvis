## Context

当前仓库已经具备三类与 memory feature 直接相关的基础能力：

- 线程初始化阶段可以生成并持久化稳定的 `System` prompt 前缀
- 非基础工具能力已经收口为按线程加载的 toolset
- 本地文件读写工具链已经存在，适合承载首版本地 memory 仓库

但当前关于 memory 的设计仍然停留在实验文本和旧 spec 设想里，存在两个问题：

- 旧方向把 memory 视为 request-time 动态注入内容，容易把“长期记忆”重新做成一次性的 prompt 拼接
- 旧方向没有定义本地持久化布局、文档格式和 tool contract，导致 memory 无法真正成为可实现的 feature

这次变更的核心约束已经明确：

- `Active Memory` 不再做正文或摘要的主动召回
- `Active Memory` 的职责是把 `keyword -> relative path` 词表作为稳定 system prompt 注入到线程初始化快照
- memory 正文、搜索和列表能力只通过 tool 返回给模型，采用渐进式披露
- `memory` 需要作为一个独立 toolset 存在，而不是继续混成临时 feature prompt

## Goals / Non-Goals

**Goals:**
- 提供本地 `./.openjarvis/memory` 仓库，支持 `active` / `passive` 两类 memory 文档。
- 定义统一的 markdown + frontmatter memory 文档格式。
- 让线程初始化过程能够从本地 active memory 文档派生关键词词表，并将其持久化为线程稳定 system prompt。
- 将 memory 能力收敛为一个可加载的 `memory` toolset，首版包含 `memory_get`、`memory_search`、`memory_write`、`memory_list`。
- 明确 tool schema 中 `type`、`path`、`keywords` 的约束，避免路径歧义与无效 active memory 数据。
- 明确舍弃“request-time 主动注入 memory 内容”，改为和 skill 类似的渐进式披露路径。

**Non-Goals:**
- 本次不实现 embedding、向量检索、FTS5 或其他高级检索机制。
- 本次不实现“关键词命中后自动把 memory 摘要或正文塞进模型”。
- 本次不为已经初始化完成的线程提供 active memory 词表热刷新能力。
- 本次不把 memory 数据并入 `SessionStore` / `Thread` 持久化聚合根。
- 本次不引入远程 memory backend、多用户共享 memory 空间或权限控制模型。

## Decisions

### 1. 引入独立的本地 `MemoryRepository`，以 `./.openjarvis/memory` 目录作为事实来源

首版 memory 不会复用 `SessionStore`、`Thread` 或 SQLite thread persistence 表，而是使用工作区下独立的本地目录：

- `./.openjarvis/memory/active/**/*.md`
- `./.openjarvis/memory/passive/**/*.md`

`MemoryRepository` 负责：

- 扫描本地 memory 目录
- 解析 frontmatter 与正文
- 构建 active keyword catalog
- 执行 list / get / search / write

这样可以保持边界清晰：

- `Thread` 继续只保存线程上下文事实
- `MemoryRepository` 保存跨线程的本地长期知识
- `memory` toolset 与 feature provider 只依赖 repository，不直接操作底层文件布局细节

Alternative considered:
- 直接把 memory 数据塞进现有 `SessionStore` / `Thread`。
  Rejected，因为 memory 是跨线程、跨重启的独立知识层，不属于线程消息聚合。

### 2. memory 文档采用 markdown + YAML frontmatter，`type` 由目录决定

首版 `MemoryDocument` 使用 markdown 文件表示，并通过 frontmatter 保存元数据：

- 所有文档都必须包含 `title`、`created_at`、`updated_at`
- `active` 文档额外必须包含 `keywords`
- `passive` 文档不得依赖 `keywords`

`type` 不写入 frontmatter，而是完全由目录推导：

- `./.openjarvis/memory/active/...` -> `active`
- `./.openjarvis/memory/passive/...` -> `passive`

这样可以避免 metadata 与目录语义重复，同时让文件移动即可改变类型边界。

Alternative considered:
- 在 frontmatter 中再存一个 `type` 字段。
  Rejected，因为和目录语义重复，且容易出现 metadata 与路径不一致的问题。

### 3. active keyword catalog 在 load 时派生，不单独持久化 index 文件

`ActiveMemoryIndexEntry` 是运行时派生结构，而不是新的持久化事实文件。系统在 load active 文档时直接从 frontmatter 的 `keywords` 构建：

- `keyword`
- `path`，相对于 `./.openjarvis/memory/active/` 根目录的相对路径

首版要求 active keyword 在整个 active memory 仓库内全局唯一。这样可以确保 system prompt 里的词表是稳定的 `keyword -> path` 一对一映射，避免模型面对同一个关键词的多路径歧义。

Alternative considered:
- 额外维护 `./.openjarvis/memory/active/index.json`。
  Rejected，因为会引入双写与索引漂移风险；首版直接从文档派生更稳。

Alternative considered:
- 允许同一个 keyword 对应多个 path。
  Rejected，因为会削弱词表的可读性与工具调用确定性。

### 4. `Active Memory` 只在 thread 初始化或重初始化时注入词表，不再做 request-time 主动 recall

这次变更明确舍弃旧的“命中关键词后自动把 memory 正文或摘要注入模型”的路径。新的行为是：

1. 系统在新线程初始化或线程被清空后重新初始化时，读取本地 active memory
2. 生成一个稳定的 memory catalog system prompt
3. 将其中的 `keyword -> path` 词表持久化到该线程的 system prefix
4. 模型后续如需正文，必须显式使用 memory 工具

这就是渐进式披露：

- system prompt 只披露存在什么记忆
- 工具调用才披露具体内容

这比 request-time 主动 recall 更容易实现，也更符合当前 `Thread` 的稳定 system snapshot 边界。

Alternative considered:
- 继续沿用旧设想，request-time 检测关键词命中后自动注入 memory 摘要或正文。
  Rejected，因为会把长期 memory 重新做成瞬时 prompt 拼接，且与当前 thread init snapshot 模型冲突。

### 5. `memory` 作为一个可加载 toolset 暴露，而不是基础 builtin tool

首版 memory 能力会注册为一个 program-defined toolset，名称固定为 `memory`。其工具集合固定为：

- `memory_get`
- `memory_search`
- `memory_write`
- `memory_list`

这样做有两个原因：

- 和现有 `thread-managed-toolsets` 机制一致，模型必须显式装载非基础能力
- 和 skill 的渐进式披露体验一致，先看到 catalog，再决定是否加载与使用能力

对应地，active memory catalog prompt 应明确提示：需要细节时先加载 `memory` toolset，再调用相关工具。

Alternative considered:
- 直接把四个 memory 工具作为始终可见的 builtin tool。
  Rejected，因为会破坏现有非基础能力的 toolset 边界，也会放大默认 visible tool list。

### 6. tool contract 统一使用 `type + type-relative path`，`memory_write` 对 active 施加额外约束

为了避免 `active` 与 `passive` 下同名路径的歧义，首版约定：

- `memory_get(path, type)` 中的 `type` 必填
- `path` 始终相对于对应 type 根目录
- 不允许绝对路径
- 不允许 `..`
- 只允许 `.md`

`memory_write` 的 contract 为：

- `memory_write(path, title, content, type="passive", keywords?)`
- `type=active` 时 `keywords` 必填且非空
- `type=passive` 时 `keywords` 禁止传入

这样可以把 active keyword 约束直接体现在 tool schema 上，减少模型生成无效数据的概率。

Alternative considered:
- 让 `memory_get` 只收 `path`，不显式传 `type`。
  Rejected，因为一旦 active/passive 存在同名相对路径，就必须引入额外猜测规则。

### 7. `memory_search` 首版采用本地词法检索，返回结构化候选而不是正文

首版 search 不做 embedding 或复杂 ranking，而是基于本地仓库对以下字段执行词法匹配：

- `title`
- `keywords`（active only）
- 正文内容
- 相对路径

`memory_search` 与 `memory_list` 返回的都是结构化候选，而不是全文。正文只允许由 `memory_get` 返回。这样可以保持渐进式披露：

- list/search 解决“有哪些”
- get 解决“具体是什么”

Alternative considered:
- `memory_search` 直接返回完整正文。
  Rejected，因为会让 search 重新变成隐式召回正文，削弱 get/list/search 的职责分层。

## Risks / Trade-offs

- [Risk] active memory 写入后，已经初始化完成的线程看不到最新词表 -> Mitigation: spec 明确 active catalog 只在线程初始化或重初始化时刷新，不做热更新。
- [Risk] memory toolset 需要先加载，模型可能多走一轮 -> Mitigation: active memory catalog prompt 里明确写出 load + get 的使用方式。
- [Risk] 词法 search 在大仓库下效果有限 -> Mitigation: 首版先把 contract 与持久化模型做稳，后续可以在不改文档格式的前提下升级为 FTS5 或其他检索实现。
- [Risk] active keyword 重复会导致 catalog 歧义 -> Mitigation: 在 write/load 阶段做唯一性校验，拒绝重复关键词。
- [Risk] 路径输入可能造成目录逃逸或类型混淆 -> Mitigation: 所有 memory tool 都统一执行相对路径规范化与 `.md` 白名单校验。

## Migration Plan

1. 新增 `memory-feature` spec 与 `thread-context-runtime` delta，先把“放弃主动注入、采用渐进式披露”的 contract 定死。
2. 引入独立的 `MemoryRepository` 模块，负责本地 memory 文档扫描、解析、写入、搜索与 active catalog 派生。
3. 新增 active memory feature provider，在 thread 初始化或重初始化时构建 memory catalog system prompt。
4. 新增 `memory` toolset 与四个工具，实现 type/path/keywords 的 schema 校验与文件系统操作。
5. 更新 worker / thread 初始化测试、memory repository UT、memory toolset UT，以及和 compact 边界相关的回归测试。

Rollback strategy:
- 如果实现过程中出现风险，可以先保留本地 `MemoryRepository` 与 `memory` toolset 设计，关闭 active memory feature provider 注入；因为 memory 数据不进入 `Thread` 聚合根，回滚影响面相对可控。

## Open Questions

- 首版是否需要额外提供一个显式“重建当前线程 active memory catalog”的命令；当前设计先不要求，默认只在初始化或重初始化时刷新。
- 当前 change 是否需要为未来 publish 版本预留全局目录切换开关；当前设计先不要求，现阶段固定使用工作区下 `./.openjarvis/memory`，后续若改为全局目录应单独开 change。
