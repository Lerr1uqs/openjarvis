## ADDED Requirements

### Requirement: Global sandbox capability policy SHALL load from config/capabilities.yaml
系统 SHALL 从 `config/capabilities.yaml` 读取面向全体用户的默认沙箱 capability 配置，而不是把该策略散落在各个 tool 或用户级上下文中。

#### Scenario: Capability file exists
- **WHEN** 进程启动且 `config/capabilities.yaml` 存在
- **THEN** 系统 SHALL 读取其中的默认沙箱后端、同步目录和限制策略，并将其用于 worker 初始化

#### Scenario: Capability file is invalid
- **WHEN** `config/capabilities.yaml` 语法错误或字段非法
- **THEN** 系统 SHALL 返回明确配置错误并阻止继续启动

### Requirement: Default synchronized workspace SHALL be the workspace root
系统 SHALL 将工作区根目录 `.` 作为默认同步目录，并把该目录作为宿主与沙箱共享的默认可写工作区。

#### Scenario: No explicit workspace sync override is provided
- **WHEN** capability 配置未覆盖同步目录
- **THEN** 系统 SHALL 使用当前工作区根目录 `.` 作为默认同步目录

#### Scenario: Sandbox writes inside the synchronized workspace
- **WHEN** 沙箱通过允许的文件原语修改工作区根目录中的文件
- **THEN** 宿主机 SHALL 能直接在同一路径观察到该变更

### Requirement: Capability policy SHALL allow explicit /tmp access
系统 SHALL 允许沙箱通过显式 `/tmp/...` 绝对路径访问宿主机 `/tmp`，以支持临时文件和临时工作目录场景。

#### Scenario: Request targets /tmp
- **WHEN** 请求路径是显式 `/tmp/...` 绝对路径
- **THEN** 系统 SHALL 允许该请求
- **THEN** 对该路径的写入 SHALL 直接反映到宿主机 `/tmp` 中

### Requirement: Capability policy SHALL restrict sensitive host paths and parent traversal
系统 SHALL 通过统一 capability 策略限制宿主敏感目录访问，并拒绝对同步工作区上级目录的访问。

#### Scenario: Request targets a sensitive host path
- **WHEN** 请求路径命中 capability 策略中的敏感宿主目录
- **THEN** 系统 SHALL 显式拒绝该请求

#### Scenario: Request escapes above the synchronized workspace
- **WHEN** 请求路径尝试通过绝对路径、`..` 或解析后的上级路径逃逸出工作区根目录，且目标也不在 `/tmp`
- **THEN** 系统 SHALL 显式拒绝该请求，而不是自动映射或透传到宿主目录
