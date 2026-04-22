## ADDED Requirements

### Requirement: Proxy SHALL compile one immutable policy snapshot for each executor launch
系统 SHALL 在每次启动 file executor、one-shot command executor 或 session executor 之前，生成一份不可变的策略快照；运行中的 executor SHALL 使用其启动时拿到的快照，而 SHALL NOT 在运行中被热更新。

#### Scenario: Proxy launches a new executor for one sandbox request
- **WHEN** proxy 收到一次需要隔离执行的文件或命令请求
- **THEN** 系统 SHALL 先根据当前授权状态生成一份新的 executor 策略快照
- **THEN** 该快照 SHALL 只作用于本次新启动的 executor

#### Scenario: Policy source update only affects future executors
- **WHEN** proxy 在一次请求完成后更新了某个 agent 的路径授权范围
- **THEN** 下一次新启动的 executor SHALL 使用更新后的快照
- **THEN** 已经运行中的 executor 或 session executor SHALL 保持原有快照边界，直到其退出或被重建

### Requirement: Policy snapshots SHALL be delivered through one-shot non-inherited IPC
系统 SHALL 通过一次性、默认不继承给最终命令子进程的 IPC 向 executor 传递策略快照，而 SHALL NOT 要求 executor 或最终命令直接读取 proxy 的长期策略源。

#### Scenario: Executor consumes a snapshot during setup
- **WHEN** proxy 启动一个新的 executor
- **THEN** executor SHALL 通过一次性 IPC 读取策略快照
- **THEN** executor 在读取完成后 SHALL 关闭该 IPC 或对应 fd

#### Scenario: Final command child cannot access the policy source
- **WHEN** command executor 已经读取快照并继续派生最终命令子进程
- **THEN** 最终命令子进程 SHALL NOT 继承策略快照 source、proxy 控制通道或其他非必要控制面 fd

### Requirement: Command executors SHALL stage dynamic Landlock before final seccomp
系统 SHALL 在 command executor 中先安装基于策略快照的动态 Landlock，再在最终命令子进程的 `exec` 前安装更严格的 seccomp，而 SHALL NOT 要求 executor 自己先安装会阻断 `fork/exec` 的最终 seccomp。

#### Scenario: Executor launches a final shell child under one snapshot
- **WHEN** one-shot command executor 或 session executor 启动最终 shell / command 子进程
- **THEN** executor SHALL 先根据快照安装动态 Landlock
- **THEN** 最终命令子进程 SHALL 在 `exec` 前安装最终 seccomp

#### Scenario: Running session executor needs a different permission set later
- **WHEN** 某个后台 session 之后需要不同的路径权限或 seccomp tier
- **THEN** 系统 SHALL 要求上层结束并重建该 session executor
- **THEN** 系统 SHALL NOT 热更新这个已在运行中的 session executor
