## ADDED Requirements

### Requirement: 系统 SHALL 将线程级 auto-compact 特性状态收口到 `ThreadContext`
系统 SHALL 将 `auto_compact` 的线程级状态与运行时决策输入收口到 `ThreadContext` 的线程状态中管理。AgentLoop 和 compact 可见性判断 SHALL 基于同一份线程状态工作；runtime compact 本身 SHALL 由全局 compact 配置控制。

#### Scenario: 循环从同一份线程状态读取 auto-compact
- **WHEN** 当前线程持久化了 `auto_compact` 状态
- **THEN** 后续该线程的 AgentLoop 会从同一个 `ThreadContext` 读取新的 auto-compact 状态
- **THEN** 系统不再依赖独立的线程 override 容器作为唯一状态来源

### Requirement: 系统 SHALL 从同一份 `ThreadContext` 派生 runtime compact 与模型侧 compact 可见性
系统 SHALL 从当前线程 `ThreadContext` 的状态与预算快照统一派生模型侧 `compact` 工具可见性；runtime compact 判断 SHALL 基于全局 compact 配置与预算快照决定，而不是分别从多个线程状态容器独立决定。

#### Scenario: compact 判断与工具可见性保持一致
- **WHEN** 当前线程的上下文预算和 compact 特性状态发生变化
- **THEN** 模型侧 `compact` 工具可见性会基于同一个 `ThreadContext` 重新计算
- **THEN** 线程不会出现“thread feature 状态已变化但模型可见性仍读取旧状态”的分裂结果
