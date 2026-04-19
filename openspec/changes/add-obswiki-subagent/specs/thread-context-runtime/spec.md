## ADDED Requirements

### Requirement: `obswiki` child thread SHALL 在初始化时注入 vault 约束与运行状态上下文
当系统初始化 `obswiki` child thread 时，除了该线程自己的基础 profile prompt 之外，系统 MUST 再写入一组稳定 `System` messages，用于说明当前绑定的 vault 约束与运行状态。这组上下文至少 MUST 包含：

- 当前 vault 路径；
- Obsidian CLI 可用性；
- QMD CLI 是否已配置且当前是否可用；
- `raw/`、`wiki/`、`schema/`、`index.md`、`AGENTS.md` 的存在性说明；
- vault `AGENTS.md` 的正文内容；
- `index.md` 中维护的 `[链接|摘要]` 索引内容。

这些信息 SHALL 作为 child thread 的稳定初始化前缀直接持久化在线程消息中，而不是通过额外的 `status` 工具按需查询。

#### Scenario: `obswiki` child thread 初始化时写入 vault 说明
- **WHEN** 系统初始化一个 `obswiki` child thread
- **THEN** 该线程的稳定 `System` 前缀会包含 vault 路径、运行状态和 `index.md` 中的链接索引
- **THEN** 该线程还会包含 vault `AGENTS.md` 的正文内容
- **THEN** 模型不需要先调用 `status` 工具才能知道当前知识库约束

#### Scenario: `obswiki` child thread 不依赖运行时 `status` 工具获取背景信息
- **WHEN** `obswiki` child thread 已完成初始化
- **THEN** 该线程可以直接基于已注入的稳定上下文理解当前 vault 结构与运行状态
- **THEN** 系统不会额外暴露 `obswiki_status` 作为模型可调用工具
