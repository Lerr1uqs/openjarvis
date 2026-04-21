## ADDED Requirements

### Requirement: Bubblewrap backend SHALL compile a kernel enforcement plan during worker initialization
当 capability policy 选择 `bubblewrap` 时，系统 SHALL 在 worker 初始化阶段把 capability 配置编译为结构化 enforcement plan，而 SHALL NOT 在 proxy 或 command child 启动时临时拼装零散参数。

#### Scenario: Worker initializes bubblewrap with kernel enforcement enabled
- **WHEN** worker 根据 capability policy 初始化 `bubblewrap` backend
- **THEN** 系统 SHALL 先编译 namespace、proxy enforcement 和 command child profile 对应的 enforcement plan
- **THEN** `bubblewrap` backend SHALL 使用该 plan 启动 proxy 与后续 child enforcement 流程

### Requirement: Bubblewrap backend SHALL fail fast when the enforcement plan cannot be satisfied
系统 SHALL 在 `bubblewrap` backend 无法满足 enforcement plan 的任何必需条件时立即失败，而 SHALL NOT 启动一个只具有部分收口能力的 sandbox。

#### Scenario: Required kernel enforcement is unavailable
- **WHEN** 当前 Linux 环境缺少 capability policy 显式要求的 namespace、Landlock ABI 或 seccomp 能力
- **THEN** worker 初始化 SHALL 失败并返回明确原因

#### Scenario: Proxy launch cannot install the baseline plan
- **WHEN** `bubblewrap` backend 无法为 proxy 安装 baseline enforcement plan
- **THEN** sandbox 初始化 SHALL 失败
- **THEN** 系统 SHALL NOT 继续保留一个未收口的 proxy 句柄

### Requirement: Bubblewrap backend SHALL pass structured enforcement state into the proxy and command child flow
系统 SHALL 把编译后的 enforcement plan 以结构化方式传入 sandbox 内 helper，而 SHALL NOT 依赖硬编码常量或只在宿主侧保留该策略。

#### Scenario: Bubblewrap backend launches the proxy with structured enforcement input
- **WHEN** `bubblewrap` backend 启动 `internal-sandbox proxy`
- **THEN** 系统 SHALL 同时传入 proxy 启动所需的 enforcement 状态

#### Scenario: Command execution selects a child profile from the runtime plan
- **WHEN** sandbox 内收到一次 `exec_command`
- **THEN** runtime SHALL 从当前 enforcement plan 中选择对应的 child profile
- **THEN** 后续 child helper SHALL 使用该 profile 完成命令启动
