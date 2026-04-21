## ADDED Requirements

### Requirement: Internal sandbox proxy SHALL accept a structured enforcement plan at startup
系统 SHALL 在启动 `internal-sandbox proxy` 时把 proxy 级 enforcement plan 传入 sandbox 内 helper，而 SHALL NOT 只依赖宿主侧本地状态推断 proxy 应该如何收口。

#### Scenario: Bubblewrap backend starts the proxy with enforcement input
- **WHEN** `bubblewrap` backend 启动 `internal-sandbox proxy`
- **THEN** proxy SHALL 接收一份结构化 enforcement plan
- **THEN** proxy SHALL 在返回握手成功前完成该 plan 的校验与应用

### Requirement: Proxy startup SHALL fail explicitly when its enforcement plan cannot be applied
系统 SHALL 在 proxy 无法应用其启动 plan 时显式失败，而 SHALL NOT 在 enforcement 失败后继续返回正常握手。

#### Scenario: Proxy cannot apply its startup enforcement plan
- **WHEN** proxy 启动阶段无法完成 `no_new_privs`、Landlock 或 baseline seccomp 所要求的动作
- **THEN** proxy SHALL 返回明确错误
- **THEN** 宿主侧握手 SHALL 失败

### Requirement: Internal sandbox helpers SHALL provide a child execution entrypoint for command enforcement
系统 SHALL 提供独立的 sandbox child execution 入口，用于在真正执行用户命令前安装 child 级 enforcement，而 SHALL NOT 继续让 proxy 直接以自身权限窗口 `exec` 用户命令。

#### Scenario: Proxy launches a child execution helper for exec_command
- **WHEN** proxy 处理一次 `exec_command`
- **THEN** proxy SHALL 通过内部 helper 入口启动 command child
- **THEN** child helper SHALL 在执行目标命令前先安装 profile 对应的 Landlock 与 Seccomp

#### Scenario: Child helper rejects an unknown command profile
- **WHEN** proxy 为命令子进程选择的 profile 不存在或无法解析
- **THEN** child helper SHALL 返回明确错误
- **THEN** proxy SHALL 把该错误显式向上返回，而不是退回为直接执行
