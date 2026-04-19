## Context

当前代码已经有几块可以直接复用的基础设施：

- `ThreadAgentKind` / `ThreadAgent` 已经是稳定的线程画像边界，适合扩展新的 subagent profile；
- `Toolset` 已经支持线程级渐进式加载；
- `browser` 已经证明了“专用 child thread + 绑定 toolset + 隐藏 CLI 调试入口”这条路径可行；
- `thread init` 已经支持在初始化阶段把稳定 `System` prompt 直接写入线程持久化前缀。

但这次需求与现有 `memory` 有明显不同：

- `memory` 明确不负责外部检索后端与向量索引，因此不适合继续承载新的知识库能力；
- `obswiki` 需要一个可插拔的、已有约束的知识库目录，而不是程序自动发明目录语义；
- 用户要求 vault 中受管理的文档操作必须经由 Obsidian CLI；
- `status`、`init`、`sync_index` 这些动作不应作为模型外显工具，而应成为运行时内建行为或注入上下文。

因此 `obswiki` 更适合做成一个独立的 subagent 能力，而不是继续扩展现有 `memory`。

## Goals / Non-Goals

**Goals:**

- 引入一个配置驱动的 `obswiki` 知识库绑定，要求显式指定 vault 路径，不在运行时自动创建或初始化陌生仓库。
- 定义标准 vault 骨架：`raw/`、`wiki/`、`schema/`、`index.md`、`AGENTS.md`。
- 让 `obswiki` child thread 在启动时自动获得 vault 运行状态、`AGENTS.md` 内容与 `index.md` 中的 `[链接|摘要]` 索引列表，不依赖额外 `status` 工具。
- 让 `obswiki` subagent 的受控工具只覆盖 `ingest / query / read / write / update` 这些核心动作，不暴露初始化、状态、日志或手动同步索引工具。
- 要求所有受管理的 vault 文档操作都通过 Obsidian CLI 或其封装层执行。
- 把 QMD 纳入本次 change，但首版只要求 QMD CLI 纯文本匹配检索，不要求 embedding。
- 先提供独立于 main agent 的调试快车道，便于先验证 tools 与 vault 接线，再决定何时让主线程主动使用它。

**Non-Goals:**

- 本次不让 main agent 默认自动选择或自动调用 `obswiki` subagent；主线程接入留到后续 change。
- 本次不实现定期 link lint / orphan link 巡检任务；它只保留为后续 TODO。
- 本次不支持非 markdown 的 Raw 输入。
- 本次不让 agent 在没有用户明确要求时自动写回 wiki 页面。
- 本次不把 Obsidian vault 的目录语义继续抽象成第二套仓库模型；规范真相以 vault 内 `AGENTS.md` 为准。
- 本次不要求 QMD embedding、向量索引或重排；只要求 QMD CLI 纯文本匹配可工作。

## Decisions

### 1. `obswiki` 使用配置驱动的外部 vault 绑定，不提供 `init` 工具

`obswiki` 的 vault 路径必须来自配置，而不是来自模型调用。程序启动或 `obswiki` subagent 初始化时必须执行 preflight：

- 解析配置中的 vault 路径；
- 校验路径存在；
- 校验 vault 根目录下存在 `AGENTS.md`、`index.md`、`raw/`、`wiki/`、`schema/`；
- 校验 Obsidian CLI 可调用。

如果这些条件不满足，系统直接失败，而不是提供 `obswiki_init` 让模型在运行时“初始化仓库”。

这样做的原因：

- 该知识库被设计成可插拔外部资产，程序只负责接入，不负责替用户发明仓库；
- 避免模型在不清楚上下文的情况下自行创建结构不一致的 vault；
- 与用户的“git clone 一个现成 vault 再接入”的使用方式一致。

Alternative considered:

- 提供 `obswiki_init()` 由 agent 运行时创建 vault。
  Rejected，因为它和“外部可插拔知识库”的目标冲突，也容易让模型在错误位置初始化出半成品目录。

### 2. vault 目录语义由 `vault/AGENTS.md` 定义，程序只校验最小骨架

程序内只固化最小通用范式：

- `raw/`
- `wiki/`
- `schema/`
- `index.md`
- `AGENTS.md`

其中：

- `raw/` 只接收导入后的 markdown 原文，后续 agent 不得改写；
- `wiki/` 存放 LLM 生成和维护的知识页；
- `schema/` 存放页面模板、更新规范、校验规则等约束；
- `index.md` 是自动维护的根索引；
- `AGENTS.md` 负责描述该知识库自己的目录职责、更新规则、渐进式披露方式和注意事项。

程序不会把更细的目录命名规则写死在代码里，而是在子线程初始化时把 `vault/AGENTS.md` 正文直接注入上下文，让 `obswiki` subagent 以该文件为一手说明。

Alternative considered:

- 把 `raw/wiki/schema/index` 的所有细节都硬编码到 Rust 模块中。
  Rejected，因为这会把本来应属于知识库资产自己的规则重新复制一份到程序里，后续会产生双重真相。

### 3. `status` 不做工具，改为 thread init 的稳定运行时上下文

用户要求 `status` 不作为工具暴露。因此 `obswiki` child thread 初始化时，系统会额外生成一段稳定上下文，至少包含：

- 已解析的 vault 路径；
- Obsidian CLI 是否可用；
- 必需骨架是否齐全；
- 当前索引文件路径；
- `index.md` 中维护的 `[链接|摘要]` 列表内容。

这段内容和 `AGENTS.md` 一样，直接写进 child thread 的稳定 `System` 前缀。这里不再把 `index.md` 重新压缩成另一份摘要，而是直接把索引里的 `[链接|摘要]` 形式喂给 LLM，让它根据链接再去显式读取页面。

Alternative considered:

- 暴露 `obswiki_status()` 让模型主动查询。
  Rejected，因为它会让模型为了拿背景信息额外走一轮工具调用，也与用户“status 应作为背景信息注入”的要求不符。

### 4. `obswiki` 的模型可见工具集只保留核心动作

首版不提供 `init/status/sync_index/log` 之类管理工具，只提供与用户任务直接相关的核心工具：

- `obswiki_import_raw(source_path, title?, source_uri?)`
- `obswiki_search(query, scope?, limit?)`
- `obswiki_read(path)`
- `obswiki_write(path, title, content, page_type?, links?, source_refs?)`
- `obswiki_update(path, instructions, expected_links?, source_refs?)`

其中：

- `obswiki_import_raw` 只接收 `.md`，落到 `raw/`，后续不可改写；
- `obswiki_search` 只返回候选，不直接替代最终回答；
- `obswiki_read` 负责显式读取正文；
- `obswiki_write` 用于新建或整体覆写一个受管页面，只允许写 `wiki/` 或 `schema/`，禁止写 `raw/`；
- `obswiki_update` 用于在已有页面上执行定向更新，语义参考当前内置文件工具里的“更新/编辑”动作，同样禁止写 `raw/`。

`index.md` 的更新由工具内部自动触发，不单独暴露 `obswiki_sync_index()`。

Alternative considered:

- 把 vault 运维动作也平铺给模型。
  Rejected，因为这会扩大工具面，并把本应固定的运行时行为暴露成不稳定的模型决策。

### 5. 所有受管理文档操作都通过 Obsidian CLI 或其封装层执行

用户要求由 Obsidian 管理的文档必须通过 Obsidian CLI 控制，因此首版设计中：

- 读取 note；
- 写回 wiki/schema 页面；
- 移动或重命名页面；
- 搜索 Obsidian 自身索引；
- 生成或刷新 `index.md`

这些动作都必须经过 Obsidian CLI 或其统一封装层，而不是绕过它直接对 vault 里的受管文件做裸 `fs::write`。

`raw` 导入也会经过统一仓库封装，但其内容落盘后即视为不可变资产，不允许后续 agent 修改。

Alternative considered:

- 只在写操作时走 Obsidian CLI，读和搜直接访问文件系统。
  Rejected，因为用户的要求是“凡是由 Obsidian 维护的文档都必须通过 Obsidian CLI 控制”，只约束写操作不够。

### 6. 检索优先使用 QMD CLI 纯文本匹配，缺失时回退到 Obsidian 搜索

`obswiki_search` 的执行策略为：

1. 如果已配置 QMD CLI 且当前可用，优先使用 QMD 执行纯文本匹配检索；
2. 如果 QMD 未配置或当前不可用，回退到 Obsidian CLI 搜索；
3. 首版不要求 embedding、向量索引或 rerank；
4. 无论使用哪个后端，返回结构都统一为候选列表，不把最终答案责任交给检索后端。

这样可以满足：

- 本次 change 内就能接入 QMD；
- 先不引入 embedding 依赖；
- 后续升级 embedding 时不需要改动对外工具契约。

Alternative considered:

- 等 embedding 配好之后再接入 QMD。
  Rejected，因为用户已经确认本次可以先以纯文本匹配方式接入 QMD。

### 7. 每次变更后自动重建 `index.md`

`index.md` 是 vault 的根索引，必须始终存在，并由系统在每次变更动作后自动刷新，包括：

- Raw 导入后；
- wiki/schema 写回后；
- 页面移动或重命名后。

模型不需要也不应该自己决定何时同步索引。

Alternative considered:

- 暴露 `obswiki_sync_index()` 交给模型按需调用。
  Rejected，因为索引完整性属于系统维护责任，不应依赖模型记得调用。

### 8. `obswiki` 先作为独立调试入口存在，不直接并入 main agent 默认流程

为了符合“先调试工具和 vault 接线，再接 subagent，再决定是否进入 main agent”的节奏，首版 change 只要求：

- 新增 `obswiki` child thread profile；
- 新增一条隐藏的本地调试入口，允许在不经过外部 channel 的情况下直接驱动 `obswiki` subagent；
- 用这条快车道验证工具行为、Obsidian CLI 接线和上下文注入。

主线程何时主动发现和使用 `obswiki` subagent，留到后续 change 再讨论。

Alternative considered:

- 直接把 `obswiki` 能力注入 main agent。
  Rejected，因为当前更高优先级是把 vault/tool/runtime 边界调通；过早接入主线程会把调试面迅速放大。

## Risks / Trade-offs

- [Risk] Obsidian CLI 的实际 headless 行为和不同安装方式存在差异 -> Mitigation: 首版通过独立调试入口先做本机验证，并把 CLI 可用性作为 preflight 条件。
- [Risk] 直接注入 `AGENTS.md` 与 `index.md` 链接列表可能增加 child thread 初始上下文长度 -> Mitigation: 首版要求 `index.md` 只维护紧凑的 `[链接|摘要]` 列表，不把页面全文塞进索引。
- [Risk] QMD CLI 与 Obsidian 搜索结果可能存在差异 -> Mitigation: 统一工具返回结构，并在 metadata 中标记 backend 来源。
- [Risk] `raw/` 不可变会让“修正导入错误”变得不方便 -> Mitigation: 首版明确把修正视为重新导入或人工整理，不在 agent 自动改写范围内。
- [Risk] vault 规则写在知识库自己的 `AGENTS.md` 中，程序只做最小校验 -> Mitigation: 把 `AGENTS.md` 作为 child thread 初始化必需文件，缺失时直接报错而不是默默忽略。

## Migration Plan

1. 新增 `add-obswiki-subagent` 的 proposal/design/specs/tasks，先把能力边界与非目标固定。
2. 扩展配置结构，增加 `agent.tool.obswiki` 运行时配置，并支持 vault 路径解析与校验。
3. 创建默认 `./.openjarvis/obswiki/` vault 骨架与 `AGENTS.md` / `index.md` / `schema` 说明文件。
4. 实现 `obswiki` runtime 与核心工具，先通过隐藏 CLI 或脚本入口直接调试。
5. 新增 `obswiki` child thread profile 与 thread init 上下文注入。
6. 在独立调试通过后，再单独决定是否发起 main agent 集成 change。

Rollback strategy:

- 如果中途发现 Obsidian CLI 或 QMD CLI 接线不稳定，可以先保留配置结构、vault 骨架与 thread init 注入设计，把 `obswiki` 执行动作限制在隐藏调试入口中，不接入正常 agent 路径。

## Open Questions

- `obswiki_update` 的输入形式首版是否更接近内置 `edit` 风格的指令式更新，还是直接要求完整新内容。
- 页面移动与重命名是否要在首版作为单独受控动作暴露，还是等 `write/update` 跑顺后再补。
