## MODIFIED Requirements

### Requirement: `auto_compact` 开启时 SHALL 允许模型主动触发 compact
当 `auto_compact` 开启时，系统 SHALL 向模型暴露 `compact` 工具，让模型可以自行选择压缩时机。上下文预算、容量提示或其他预警信息如果存在，SHALL 仅用于帮助模型判断是否应尽快调用 `compact`，而 SHALL NOT 作为该工具是否可见的前置条件。

#### Scenario: 开启 auto-compact 后模型立即可见 compact tool
- **WHEN** 某个线程已经启用 `auto_compact`
- **THEN** 当前模型请求中可见 `compact` 工具
- **THEN** 模型无需等待上下文预算达到额外阈值才获得该工具

#### Scenario: 容量提示不会决定 compact tool 是否存在
- **WHEN** `auto_compact` 已开启，且系统同时提供上下文容量提示或预算报告
- **THEN** 这些提示只帮助模型判断“是否应该尽快 compact”
- **THEN** `compact` 工具是否出现在工具列表中不再由可见阈值控制
