## ADDED Requirements

### Requirement: 系统 SHALL 将 subagent 建模为主线程可启用的正式 feature
系统 SHALL 将 subagent 从“只有工具”的隐式能力提升为主线程可启用的正式 feature。该 feature SHALL 负责主线程对 subagent 的稳定认知，包括能力摘要、使用时机说明，以及对应管理工具的可见性边界。child thread SHALL NOT 被视为该 feature 的消费者。

#### Scenario: 主线程启用 subagent feature 后获得完整能力认知
- **WHEN** 某个主线程启用了 subagent feature
- **THEN** 该线程初始化时会获得一段稳定的 subagent system prompt
- **THEN** 该线程后续可见工具中包含 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent`

#### Scenario: child thread 不消费 subagent feature
- **WHEN** 系统初始化一个 `browser` child thread
- **THEN** 该 child thread 不会获得“有哪些 subagent 可调用”的父线程管理说明
- **THEN** 该 child thread 不会看到 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent`

### Requirement: 系统 SHALL 在主线程初始化时注入稳定的 subagent feature prompt
系统 SHALL 在主线程初始化或重初始化时构建一段稳定的 subagent feature prompt，并将其作为该主线程 `System` 前缀的一部分持久化。该 prompt SHALL 至少包含：

- 当前可用 subagent 的总数
- 每个 subagent 的 `subagent_key`
- 每个 subagent 的职责摘要
- 每个 subagent 的推荐使用场景
- 每个 subagent 的不推荐使用场景

#### Scenario: 初始化 prompt 显示当前可用 subagent 数量
- **WHEN** 当前运行时只有一个可用 subagent profile `browser`
- **THEN** 新主线程初始化后的 subagent feature prompt 会明确说明当前可用 subagent 数量为 `1`
- **THEN** 该 prompt 会列出 `browser` 及其职责摘要

### Requirement: subagent feature prompt SHALL 明确说明“什么时候用”和“什么时候不用”
系统 SHALL 在 subagent feature prompt 中明确提供使用边界，而 SHALL NOT 只给出 profile 名称或泛化描述。至少需要覆盖以下原则：

- 当任务明显属于某个专用 profile，且需要独立子线程上下文时，应该使用 subagent
- 当任务可由主线程直接完成，或只需要一次简单工具调用时，不应该为了形式感额外使用 subagent
- 当某个 profile 已存在可复用 child thread 时，应优先复用，而不是新起额外并行实例

#### Scenario: prompt 提醒简单任务不必起 subagent
- **WHEN** 主线程查看初始化后的 subagent feature prompt
- **THEN** prompt 中会明确说明“简单直接的工具调用不应默认升级成 subagent 调用”
- **THEN** prompt 中会明确说明“只有在需要专用职责和隔离上下文时才应优先使用 subagent”

### Requirement: subagent feature prompt SHALL 基于当前可用 subagent catalog 生成
系统 SHALL 基于当前运行时实际可用的 subagent catalog 构建 subagent feature prompt，而 SHALL NOT 把可用 subagent 列表永久硬编码在基础主线程 prompt 中。当前可用 subagent 数量、profile 列表和职责说明 SHALL 随 catalog 变化而变化，并在后续新线程初始化或重初始化时生效。

#### Scenario: 可用 subagent catalog 变化后后续线程看到更新内容
- **WHEN** 某次运行时当前可用 subagent 从只有 `browser` 扩展为 `browser` 与另一个新 profile
- **THEN** 后续新建或重初始化的主线程看到的 subagent feature prompt 会反映新的总数和 profile 列表
- **THEN** 系统不会要求把这些 profile 名字硬编码到基础主线程 prompt 中

### Requirement: subagent 管理工具 SHALL 由 subagent feature 拥有可见性
系统 SHALL 将 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 视为 subagent feature 拥有的主线程能力。若某个主线程未启用 subagent feature，系统 SHALL NOT 向该线程暴露这组工具。

#### Scenario: feature 关闭时主线程看不到 subagent 管理工具
- **WHEN** 某个主线程未启用 subagent feature
- **THEN** 该线程初始化时不会获得 subagent feature prompt
- **THEN** 该线程后续可见工具中不会包含 `spawn_subagent`
- **THEN** 该线程后续可见工具中不会包含 `send_subagent`
- **THEN** 该线程后续可见工具中不会包含 `close_subagent`
- **THEN** 该线程后续可见工具中不会包含 `list_subagent`
