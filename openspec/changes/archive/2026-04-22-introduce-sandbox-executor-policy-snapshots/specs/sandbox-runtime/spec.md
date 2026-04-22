## ADDED Requirements

### Requirement: Bubblewrap runtime SHALL separate a long-lived proxy control plane from policy-scoped executors
当 capability policy 选择 `bubblewrap` 时，系统 SHALL 维持一个长生命周期的 sandbox proxy 作为控制面，并把真正的文件动作与命令动作交给按请求或按 session 派生的 executor，而 SHALL NOT 继续让单个长生命周期 proxy 持有主要工作区读写和最终命令执行能力。

#### Scenario: Worker initializes bubblewrap with executor-based routing
- **WHEN** worker 根据 capability policy 初始化 `bubblewrap` backend
- **THEN** 系统 SHALL 启动一个长生命周期 proxy 作为当前 sandbox runtime 的控制面
- **THEN** 后续文件和命令请求 SHALL 通过该 proxy 派生相应 executor

#### Scenario: One worker serves both file and command requests
- **WHEN** 同一个 sandbox runtime 先后接收文件请求与命令请求
- **THEN** proxy SHALL 能为文件请求启动 file executor
- **THEN** proxy SHALL 能为命令请求启动 one-shot command executor 或 session executor

### Requirement: Bubblewrap runtime policy updates SHALL only affect future executors
系统 SHALL 将 sandbox 的动态授权更新定义为“影响未来 executor 启动时的策略快照”，而 SHALL NOT 试图热更新已运行中的 executor 或 session executor。

#### Scenario: Upper layer updates one agent's writable paths before the next request
- **WHEN** 上层在一次请求完成后更新了某个 agent 可写路径集合
- **THEN** 下一次由 proxy 启动的新 executor SHALL 使用更新后的策略快照
- **THEN** 已经运行中的 executor SHALL 不会被热更新

#### Scenario: Running session needs a new permission envelope
- **WHEN** 某个后台 session 需要超出其原快照的新路径权限
- **THEN** 系统 SHALL 要求上层重建该 session executor
- **THEN** 系统 SHALL NOT 在原 session executor 内直接放宽权限

### Requirement: Bubblewrap runtime SHALL fail fast when executor bootstrap cannot satisfy required isolation
系统 SHALL 在 executor 无法完成策略快照读取、Landlock 安装、最终 seccomp 安装或必要 fd 收口时显式失败，而 SHALL NOT 回退到 proxy 直接执行或宿主机直接执行。

#### Scenario: File executor cannot install its required Landlock rules
- **WHEN** proxy 为某次文件请求启动的 file executor 无法安装快照要求的 Landlock 规则
- **THEN** 本次请求 SHALL 显式失败
- **THEN** 系统 SHALL NOT 改由 proxy 自己直接执行该文件动作

#### Scenario: Command executor cannot hand off a properly restricted final child
- **WHEN** command executor 无法在最终命令 `exec` 前完成必须的 seccomp 安装或 fd 收口
- **THEN** 本次命令启动 SHALL 显式失败
- **THEN** 系统 SHALL NOT 回退到宽权限路径继续执行
