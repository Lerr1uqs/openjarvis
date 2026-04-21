## ADDED Requirements

### Requirement: Capability policy SHALL declare bubblewrap kernel enforcement profiles
系统 SHALL 允许 `config/capabilities.yaml` 在 `bubblewrap` 配置下声明 namespace 开关、baseline seccomp profile、proxy Landlock profile 与 command child profile，而 SHALL NOT 继续只表达同步目录与敏感路径限制。

#### Scenario: Capability file declares kernel enforcement profiles
- **WHEN** `config/capabilities.yaml` 为 `bubblewrap` backend 提供内核 enforcement 配置
- **THEN** 系统 SHALL 读取并校验这些 profile，并将其用于 worker 初始化阶段的 enforcement plan 编译

#### Scenario: Capability file references an unknown profile
- **WHEN** capability 配置引用不存在、空白或不完整的 seccomp / Landlock / command profile
- **THEN** 系统 SHALL 返回明确配置错误并阻止继续启动

### Requirement: Capability policy SHALL declare bubblewrap namespace intent explicitly
系统 SHALL 允许 capability policy 显式声明 `bubblewrap` 需要启用的 namespace 组合，例如 user、pid、ipc、uts、net，而 SHALL NOT 把这些选择硬编码在实现中。

#### Scenario: Capability file enables namespace switches
- **WHEN** capability 配置为 `bubblewrap` backend 声明 namespace 开关
- **THEN** 系统 SHALL 将这些开关编译进 `bubblewrap` 启动计划

### Requirement: Capability policy SHALL support fail-closed kernel compatibility requirements
系统 SHALL 允许 capability policy 声明对 Landlock ABI、seccomp 能力与相关内核支持的显式要求；当这些要求不满足时，系统 SHALL fail fast，而 SHALL NOT 以较弱隔离继续运行。

#### Scenario: Policy requires a minimum Landlock ABI
- **WHEN** capability policy 显式要求最小 Landlock ABI，且当前内核不满足该要求
- **THEN** worker 初始化 SHALL 失败并报告兼容性错误

#### Scenario: Policy requires seccomp support
- **WHEN** capability policy 显式要求 baseline seccomp 或 child seccomp 能力，且当前环境无法安装该 profile
- **THEN** worker 初始化 SHALL 失败，而不是静默跳过该层 enforcement
