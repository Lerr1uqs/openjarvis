## ADDED Requirements

### Requirement: 系统 SHALL 提供可独立实例化的 `ContextCompactor` component
系统 SHALL 提供一个可独立于 `AgentLoop` 实例化的 `ContextCompactor` component。调用方 SHALL 可以在 loop 之外显式创建该 component，并在任意合适的编排位置串联调用它，而不需要先进入 AgentLoop 内部执行路径。

#### Scenario: 外部调用方可以直接初始化 compactor
- **WHEN** 某个调用方需要在 loop 外部对某个线程执行 compact
- **THEN** 它可以直接初始化一个 `ContextCompactor`
- **THEN** 它不需要通过 `AgentLoop` 的成员方法间接触发 compact

### Requirement: `ContextCompactor` SHALL 对传入线程消息执行 compact 并返回标准 outcome
`ContextCompactor` SHALL 接收待 compact 的线程消息视图或线程快照，并对其执行 compact。执行完成后，component SHALL 返回与现有 compact 语义兼容的标准 compact outcome，其中包含 compact 是否发生、replacement thread / turn 以及相关摘要结果。

#### Scenario: compactor 返回 compact 结果供调用方继续串联
- **WHEN** 调用方把某个线程快照交给 `ContextCompactor`
- **THEN** compactor 会返回标准 compact outcome 或“无需 compact”的结果
- **THEN** 调用方可以继续使用 compact 后的线程消息发起后续 LLM 请求

### Requirement: `ContextCompactor` SHALL 支持显式注入 compact strategy 与 provider
`ContextCompactor` SHALL 支持显式注入 compact strategy 与 compact provider，以便调用方在不修改 AgentLoop 的前提下独立构造不同的 compact 组合。

#### Scenario: 调用方为 compactor 指定不同 strategy
- **WHEN** 调用方在初始化 `ContextCompactor` 时提供特定 compact strategy 或 provider
- **THEN** compactor 会按该显式配置执行 compact
- **THEN** AgentLoop 不需要成为这些 compact 依赖的唯一持有者

### Requirement: `ContextCompactor` SHALL 保持 loop 无关的纯 compact 职责
`ContextCompactor` SHALL 只负责 compact 本身的执行与结果产出，而 SHALL NOT 直接承担 Router 事件发送、loop 消息缓存维护或 turn 存储副作用。外围调用方 SHALL 基于 compactor 返回结果自行决定后续接线行为。

#### Scenario: compactor 不直接操作 loop 状态
- **WHEN** 某个 compact 调用成功完成
- **THEN** `ContextCompactor` 只返回 compact 结果
- **THEN** 是否覆写线程 active history、是否记录事件以及何时继续请求 LLM 由调用方决定
