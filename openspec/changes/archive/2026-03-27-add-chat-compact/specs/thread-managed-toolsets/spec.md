## ADDED Requirements

### Requirement: 系统 SHALL 支持线程运行时的条件化工具可见性
系统 SHALL 在 thread-scoped tool projection 阶段支持条件化工具可见性。某个工具即使已经注册，也 SHALL 可以根据线程运行时状态、配置开关或上下文预算决定当前是否对模型可见。

#### Scenario: 工具已注册但当前线程不可见
- **WHEN** 某个工具已经注册到工具运行时，但其当前线程可见性条件不满足
- **THEN** 该工具不会出现在当前线程发送给模型的 visible tool list 中
- **THEN** 其他满足条件的线程仍然可以独立看到该工具

### Requirement: Agent loop SHALL 在每次生成前刷新工具可见性投影
系统 SHALL 在每次模型生成前重新计算当前线程的 visible tool projection，而不是只基于静态注册结果。该刷新 SHALL 同时考虑已加载 toolset 和条件化工具显隐。

#### Scenario: 上下文预算变化影响工具可见性
- **WHEN** 当前线程的上下文预算或特性开关发生变化
- **THEN** 下一次模型生成前会重新计算该线程的 visible tool list
- **THEN** 条件满足的工具可以在同一线程后续步骤中出现或消失
