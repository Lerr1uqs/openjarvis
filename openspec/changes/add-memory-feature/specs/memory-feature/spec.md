## ADDED Requirements

### Requirement: 系统 SHALL 使用本地 `./.openjarvis/memory/{active,passive}` markdown 仓库持久化 memory
系统 SHALL 在工作区根目录下使用本地 `./.openjarvis/memory` 目录作为 memory 事实来源，并按以下固定布局保存文档：

- `./.openjarvis/memory/active/**/*.md`
- `./.openjarvis/memory/passive/**/*.md`

文档类型 SHALL 仅由目录决定，而 SHALL NOT 依赖绝对路径或额外 `type` metadata 推断。

#### Scenario: passive memory 写入到本地仓库
- **WHEN** 调用方成功执行一次 `memory_write(path="notes/user-preference.md", title=..., content=...)`
- **THEN** 系统会把文档写入 `./.openjarvis/memory/passive/notes/user-preference.md`
- **THEN** 该文档类型会被视为 `passive`

### Requirement: memory 文档 SHALL 使用 frontmatter metadata，且 active 文档 MUST 声明 `keywords`
系统 SHALL 使用 markdown frontmatter 作为 memory 文档 metadata 格式。所有 memory 文档的 frontmatter SHALL 至少包含：

- `title`
- `created_at`
- `updated_at`

`active` 文档的 frontmatter 还 SHALL 包含非空 `keywords` 数组；`passive` 文档 SHALL NOT 依赖 `keywords` 参与索引或行为判定。

#### Scenario: active memory 文档在 load 时生成关键词索引项
- **WHEN** 系统加载一个 `./.openjarvis/memory/active/workflow/notion.md` 文档，且其 frontmatter 包含 `keywords: [\"notion\", \"上传\"]`
- **THEN** 系统会基于该 frontmatter 派生出 `keyword -> relative path` 的 active memory 索引项
- **THEN** 派生索引项中的 path 为相对 `./.openjarvis/memory/active/` 根目录的 `workflow/notion.md`

### Requirement: active memory keyword 映射 SHALL 在 active 仓库内保持唯一
系统 SHALL 将 active memory 词表视为稳定的 `keyword -> path` 单值映射。若两个不同 active 文档声明了同一个 keyword，系统 SHALL 将其视为无效状态并拒绝生成歧义 catalog。

#### Scenario: 重复 keyword 会导致 active catalog 构建失败
- **WHEN** `./.openjarvis/memory/active/a.md` 与 `./.openjarvis/memory/active/b.md` 都声明了 keyword `notion`
- **THEN** 系统不会生成包含歧义 keyword 的 active memory catalog
- **THEN** 调用方会收到明确的重复 keyword 错误

### Requirement: 系统 SHALL 在 thread 初始化或重初始化时注入 active memory catalog system prompt
系统 SHALL 在新线程初始化或线程被清空后重新初始化时，从本地 active memory 文档构建一个稳定的 active memory catalog system prompt，并将其作为线程初始化 system snapshot 的一部分持久化到该线程。该 catalog SHALL 只披露 `keyword -> relative path` 词表以及 memory 工具使用提示，而 SHALL NOT 直接包含 memory 正文。

#### Scenario: 新线程初始化时看到 active memory 词表
- **WHEN** 本地 active memory 仓库中存在 `workflow/notion.md`，且其关键词包含 `notion`
- **THEN** 新线程初始化后的 system prompt 中会包含 `notion -> workflow/notion.md` 这样的词表项
- **THEN** 该 prompt 会提示模型需要时通过 memory 工具读取正文，而不是直接给出正文内容

### Requirement: 系统 SHALL 舍弃主动正文注入并采用渐进式披露
系统 SHALL NOT 因用户输入命中 active memory 关键词而自动向模型注入对应 memory 的正文、摘要或其他 request-time recall message。active memory 的默认行为 SHALL 是先通过 system prompt 披露词表，再由模型按需调用 memory 工具逐步读取详情。

#### Scenario: 关键词命中不会自动把正文塞进请求
- **WHEN** 当前线程的 user input 中包含某个 active memory keyword
- **THEN** 系统不会因为这次命中而额外向模型自动注入该 memory 文档的正文或摘要
- **THEN** 模型只有在显式调用 memory 工具后才能拿到该文档内容

### Requirement: 系统 SHALL 提供可线程加载的 `memory` toolset
系统 SHALL 将 memory 能力注册为一个 program-defined toolset，名称固定为 `memory`。当前线程加载该 toolset 后，模型可见工具 SHALL 至少包括：

- `memory_get`
- `memory_search`
- `memory_write`
- `memory_list`

#### Scenario: 加载 `memory` toolset 后 memory 工具可见
- **WHEN** 当前线程成功加载 `memory` toolset
- **THEN** 当前线程后续可见工具列表中包含 `memory_get`、`memory_search`、`memory_write` 和 `memory_list`
- **THEN** 未加载 `memory` toolset 的其他线程不会看到这些工具

### Requirement: `memory_write` SHALL 按 type-relative path 写入文档并校验 active keyword 约束
系统 SHALL 提供 `memory_write(path, title, content, type=\"passive\", keywords?)` 工具。其行为约束如下：

- `path` SHALL 是相对于对应 type 根目录的相对路径
- `path` SHALL NOT 是绝对路径
- `path` SHALL NOT 包含 `..`
- `path` SHALL 以 `.md` 结尾
- `type=active` 时 `keywords` SHALL 必填且非空
- `type=passive` 时 `keywords` SHALL NOT 被接受

#### Scenario: active memory 写入时必须同时写入 keywords
- **WHEN** 调用方执行 `memory_write(path=\"workflow/notion.md\", title=..., content=..., type=\"active\", keywords=[\"notion\"])`
- **THEN** 系统会把文档写入 `./.openjarvis/memory/active/workflow/notion.md`
- **THEN** 该文档的 frontmatter 中包含 `keywords`
- **THEN** 若缺少 `keywords`，该调用会失败

### Requirement: `memory_get` SHALL 通过 `type + path` 读取单个 memory 文档
系统 SHALL 提供 `memory_get(path, type)` 工具。`type` SHALL 为必填参数，`path` SHALL 相对于对应 type 根目录解析。系统 SHALL 返回目标文档的 metadata 与正文内容；若目标不存在或路径非法，调用 SHALL 失败。

#### Scenario: 通过 `type + path` 读取 active memory 正文
- **WHEN** 调用方执行 `memory_get(path=\"workflow/notion.md\", type=\"active\")`
- **THEN** 系统会读取 `./.openjarvis/memory/active/workflow/notion.md`
- **THEN** 返回结果中包含该文档的 `title`、时间 metadata 和正文内容

### Requirement: `memory_search` 与 `memory_list` SHALL 返回结构化候选，而不是正文注入
系统 SHALL 提供 `memory_search(query, type?, limit?)` 与 `memory_list(type?, dir?)` 工具。两者返回值 SHALL 以结构化候选列表为主，至少包含每个候选的 `type`、`path` 与 `title`；其中 `memory_search` 还 SHALL 支持按 `type` 过滤，`memory_list` SHALL 支持按目录前缀列出文档。两者 SHALL NOT 直接返回完整正文作为默认结果。

#### Scenario: `memory_search` 返回候选列表后再由 `memory_get` 读取正文
- **WHEN** 调用方执行 `memory_search(query=\"notion\", type=\"active\", limit=5)`
- **THEN** 系统返回的是匹配文档的候选列表，而不是完整正文
- **THEN** 调用方可以基于返回的 `type + path` 再调用 `memory_get` 读取具体内容
