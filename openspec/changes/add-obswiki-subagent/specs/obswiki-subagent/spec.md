## ADDED Requirements

### Requirement: 系统 SHALL 通过配置绑定一个可插拔的 Obsidian vault
系统 SHALL 通过显式配置绑定一个 `obswiki` Obsidian vault。该配置至少 MUST 指定 vault 路径，并 MAY 指定 Obsidian CLI 相关运行时参数。系统在启动或 `obswiki` subagent 初始化时 MUST 校验该路径存在且满足最小骨架要求；若 vault 路径不存在、不可访问或骨架缺失，系统 MUST 直接失败，而不是在运行时提供单独的初始化工具。

#### Scenario: 已配置且存在的 vault 通过 preflight 校验
- **WHEN** 配置中的 `obswiki` vault 路径存在且包含必需骨架
- **THEN** 系统会接受该 vault 作为当前 `obswiki` 知识库
- **THEN** 后续 `obswiki` subagent 与工具都以这个 vault 为事实来源

#### Scenario: 缺少 vault 路径时直接报错
- **WHEN** `obswiki` 已启用但配置中的 vault 路径不存在
- **THEN** 系统会直接返回配置或初始化错误
- **THEN** 系统不会暴露 `obswiki_init` 一类运行时初始化工具

### Requirement: 系统 SHALL 要求 vault 满足标准骨架并以 `AGENTS.md` 作为知识库说明真相
`obswiki` vault 根目录 MUST 至少包含 `raw/`、`wiki/`、`schema/`、`index.md` 和 `AGENTS.md`。其中 `AGENTS.md` MUST 说明该知识库的目录职责、渐进式披露方式和写回约束；系统 MAY 对更细的目录结构保持开放，但 MUST 以上述骨架作为最小通用范式。

#### Scenario: 标准骨架存在时 vault 被接受
- **WHEN** vault 根目录存在 `raw/`、`wiki/`、`schema/`、`index.md` 和 `AGENTS.md`
- **THEN** 系统会认定该 vault 满足最小接入条件
- **THEN** `AGENTS.md` 会被视为该知识库目录职责和操作规则的一手说明

#### Scenario: 缺少 `AGENTS.md` 时拒绝启动 `obswiki`
- **WHEN** vault 根目录缺少 `AGENTS.md`
- **THEN** 系统会拒绝启动或初始化 `obswiki` subagent
- **THEN** 系统不会用程序内默认假设去替代该知识库自己的说明文件

### Requirement: Raw 层 SHALL 只接收 markdown 摄入且后续不可被 agent 改写
系统 MUST 提供 `obswiki_import_raw(source_path, title?, source_uri?)` 形式的 Raw 摄入能力。Raw 摄入只接受 markdown 输入，并将结果存入 vault 的 `raw/` 层。对已经落入 `raw/` 的受管文档，agent SHALL NOT 通过任何写回工具再次修改。

#### Scenario: markdown 文件可被导入到 `raw/`
- **WHEN** subagent 调用 `obswiki_import_raw` 且 `source_path` 指向一个 markdown 文件
- **THEN** 系统会把该文档导入到 vault 的 `raw/` 层
- **THEN** 该导入结果会成为后续 wiki 整理和查询的原始来源之一

#### Scenario: Raw 文档不能通过写回工具被改写
- **WHEN** subagent 尝试把 `obswiki_write` 或 `obswiki_update` 目标写到 `raw/` 路径
- **THEN** 系统会拒绝该操作
- **THEN** 系统不会允许 agent 改写已导入的 Raw 文档

### Requirement: 系统 SHALL 仅暴露核心 `obswiki` 工具，而不暴露初始化或状态管理工具
`obswiki` 子线程内模型可见的工具集 MUST 只覆盖核心业务动作。首版系统 MUST 暴露 `obswiki_import_raw`、`obswiki_search`、`obswiki_read`、`obswiki_write` 和 `obswiki_update`。系统 SHALL NOT 暴露 `obswiki_init`、`obswiki_status`、`obswiki_sync_index` 或 `obswiki_append_log` 作为模型可调用工具。

#### Scenario: `obswiki` 子线程只看到核心工具
- **WHEN** 一个 `obswiki` child thread 完成初始化并查看可用工具
- **THEN** 它能看到 `obswiki_import_raw`、`obswiki_search`、`obswiki_read`、`obswiki_write` 和 `obswiki_update`
- **THEN** 它看不到 `obswiki_init`、`obswiki_status`、`obswiki_sync_index` 或 `obswiki_append_log`

### Requirement: `obswiki_search` SHALL 优先使用 QMD CLI 纯文本匹配并返回结构化候选
`obswiki_search` MUST 支持双后端执行策略：当 QMD CLI 已配置且当前可用时，系统 SHALL 优先使用 QMD 执行纯文本匹配检索；当 QMD CLI 未配置或当前不可用时，系统 SHALL 回退到 Obsidian CLI 搜索。首版系统 SHALL NOT 要求 embedding、向量索引或 rerank。该工具返回 MUST 是结构化候选，而不是最终回答。

#### Scenario: QMD CLI 可用时优先使用纯文本匹配
- **WHEN** subagent 调用 `obswiki_search` 且 QMD CLI 已配置并可用
- **THEN** 系统会优先通过 QMD 执行纯文本匹配检索
- **THEN** 返回结果中可以包含当前 backend 来源

#### Scenario: QMD CLI 不可用时回退到 Obsidian 搜索
- **WHEN** subagent 调用 `obswiki_search` 且 QMD CLI 未配置或当前不可用
- **THEN** 系统会回退到 Obsidian CLI 搜索
- **THEN** subagent 仍需自行读取页面并综合回答

### Requirement: `obswiki_write` 与 `obswiki_update` SHALL 分别承担整页写入与定向更新
系统 MUST 将页面写回拆分为 `obswiki_write` 与 `obswiki_update` 两个动作。`obswiki_write` 用于新建或整体覆写一个受管页面；`obswiki_update` 用于对已有页面执行定向修改，语义参考当前内置文件工具中的写入/更新区分。二者都 SHALL 只允许写入 `wiki/` 或 `schema/`，并 SHALL NOT 允许写入 `raw/`。

#### Scenario: `obswiki_write` 可以新建 wiki 页面
- **WHEN** subagent 调用 `obswiki_write` 且目标路径位于 `wiki/`
- **THEN** 系统可以创建或整体覆写该页面
- **THEN** 该操作成功后会触发 `index.md` 自动刷新

#### Scenario: `obswiki_update` 不能修改 Raw 页面
- **WHEN** subagent 调用 `obswiki_update` 且目标路径位于 `raw/`
- **THEN** 系统会拒绝该操作
- **THEN** Raw 层内容仍保持不可变

### Requirement: 系统 SHALL 通过 Obsidian CLI 管理受管文档并在每次变更后自动更新 `index.md`
对于 vault 中由 Obsidian 管理的受管文档，系统 MUST 通过 Obsidian CLI 或其统一封装层执行读取、搜索、写入、更新、移动与索引刷新；系统 SHALL NOT 绕过 Obsidian 直接对这些受管文档执行裸文件操作。每次成功执行 Raw 摄入或 wiki/schema 写入更新后，系统 MUST 自动更新 `index.md`。

#### Scenario: 写回 wiki 页面后自动刷新 `index.md`
- **WHEN** `obswiki_write` 或 `obswiki_update` 成功写入 `wiki/` 或 `schema/` 下的页面
- **THEN** 系统会自动触发 `index.md` 刷新
- **THEN** 模型不需要再额外调用手动同步索引工具

#### Scenario: 读取受管页面时不绕过 Obsidian 封装
- **WHEN** subagent 调用 `obswiki_read` 读取一个受管页面
- **THEN** 系统会通过 Obsidian CLI 或其统一封装层执行读取
- **THEN** 系统不会直接把该动作实现成对受管页面的裸文件读取

### Requirement: 系统 SHALL 提供独立于 main agent 的 `obswiki` 调试入口
系统 MUST 提供一个隐藏或内部使用的 `obswiki` 调试入口，使开发者在不经过 main agent 默认流程的情况下，直接驱动 `obswiki` child thread 完成工具调用与上下文验证。该入口 MUST 与正式 `obswiki` 子线程共享同一套配置、vault 约束与工具契约。

#### Scenario: 通过独立入口调试 `obswiki` child thread
- **WHEN** 开发者使用内部调试入口启动 `obswiki`
- **THEN** 系统会按正式 `obswiki` 子线程的方式装载配置、上下文与工具
- **THEN** 开发者可以在不接入 main agent 的前提下验证 `obswiki` 行为
