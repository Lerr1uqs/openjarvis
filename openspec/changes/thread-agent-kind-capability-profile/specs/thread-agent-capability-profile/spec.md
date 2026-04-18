## ADDED Requirements

### Requirement: 系统 SHALL 为每个 `ThreadAgentKind` 定义统一 capability profile
系统 SHALL 为每个 `ThreadAgentKind` 定义一份统一 capability profile，用于描述该 kind 的线程初始化与运行时能力边界。该 profile SHALL 至少覆盖：

- 该 kind 使用的稳定 system prompt
- 该 kind 默认绑定的工具能力
- 该 kind 允许使用的 toolset 范围
- 该 kind 默认启用的 feature
- 该 kind 允许启用的 feature 范围

#### Scenario: `Main` 与 `Browser` 拥有不同 profile
- **WHEN** 系统分别解析 `ThreadAgentKind::Main` 与 `ThreadAgentKind::Browser`
- **THEN** 两者会得到不同的 capability profile
- **THEN** `Main` profile 与 `Browser` profile 的 prompt、工具和 feature 边界不会共享同一份真相

### Requirement: capability profile SHALL 来自仓库内置静态清单
系统 SHALL 从仓库内置的静态清单 `config/agents.yaml` 解析各个 `ThreadAgentKind` 的 capability profile。该清单 SHALL 随程序一同发布并固定存在，系统 SHALL NOT 支持在运行时动态覆写其中的 prompt、feature、visible tool 或 toolset 边界。

#### Scenario: 系统从静态清单加载 `Main` 与 `Browser`
- **GIVEN** 仓库内置静态清单 `config/agents.yaml`
- **WHEN** 系统首次解析 thread agent catalog
- **THEN** `Main` 与 `Browser` 的 capability profile 都来自该静态清单
- **THEN** 系统不会额外叠加运行时 profile 覆写来源

### Requirement: capability profile SHALL 成为线程能力边界真相
系统 SHALL 将 `ThreadAgentKind` 对应的 capability profile 视为线程能力边界真相，而 SHALL NOT 只把它当作初始化默认值。若某个 tool、toolset 或 feature 不在当前 kind profile 允许范围内，系统 SHALL NOT 在该线程后续运行时向模型暴露该能力。

#### Scenario: 不允许的能力不会在运行时重新出现
- **WHEN** 一个 `Browser` 线程的 kind profile 不允许 `memory` feature
- **THEN** 该线程初始化时不会注入 memory feature prompt
- **THEN** 该线程后续运行时也不会重新看到由 `memory` feature 拥有的工具能力

### Requirement: 系统 SHALL 区分 kind 默认绑定工具与可选 toolset
系统 SHALL 区分“某个 kind 默认绑定的工具能力”和“线程后续可显式加载的可选 toolset”。默认绑定工具属于 kind profile 真相的一部分；可选 toolset 仅能来自当前 kind profile 明确允许的范围。

#### Scenario: `Browser` 默认拥有浏览器能力但没有可选 skill toolset
- **WHEN** 系统初始化一个 `Browser` 线程
- **THEN** 该线程会直接拥有其 kind 默认绑定的浏览器工具能力
- **THEN** 该线程不会因此自动获得 `skill` toolset 或 `load_skill` 能力

#### Scenario: `Main` 通过 subagent 使用浏览器能力而不是直接拥有 browser toolset
- **WHEN** 系统解析 `ThreadAgentKind::Main` 的 capability profile
- **THEN** 该 profile 不会把 `browser` 视为 `Main` 可直接使用的 toolset
- **THEN** 该 profile 仍可以通过 `subagent` 相关 feature / tool 暴露调度 `Browser` kind child thread 的能力
