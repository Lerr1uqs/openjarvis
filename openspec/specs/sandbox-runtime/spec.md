## ADDED Requirements

### Requirement: Agent worker SHALL own a real sandbox runtime abstraction
系统 SHALL 用统一 `Sandbox` trait 建模沙箱运行时，并让 `AgentWorker` 持有真实沙箱实例，而不是只持有占位对象。

#### Scenario: Worker initializes a bubblewrap sandbox backend
- **WHEN** 全局 capability 配置选择 `bubblewrap`
- **THEN** `AgentWorker` SHALL 初始化 Bubblewrap 后端实例并把它作为当前 worker 的沙箱运行时

#### Scenario: Worker exposes backend kind for diagnostics
- **WHEN** 外部诊断或测试读取当前 worker 的 sandbox kind
- **THEN** 系统 SHALL 返回稳定后端标识，例如 `bubblewrap` 或 `docker`

### Requirement: Sandbox runtime SHALL keep Docker as a declared backend
系统 SHALL 在后端枚举和工厂层保留 Docker 支持入口，即使当前阶段尚未真正实现 Docker 运行时。

#### Scenario: Docker backend is selected
- **WHEN** 全局 capability 配置选择 `docker`
- **THEN** 系统 SHALL 返回明确的未实现错误，而不是静默回退到其他后端

### Requirement: Bubblewrap backend SHALL fail fast when unavailable
当配置显式选择 Bubblewrap 时，系统 SHALL 在平台不支持、`bwrap` 缺失或 proxy 启动失败时立即返回明确初始化错误。

#### Scenario: Host platform does not support bubblewrap
- **WHEN** 当前运行平台不满足 Bubblewrap 后端要求
- **THEN** worker 初始化 SHALL 失败并报告 Bubblewrap 后端不可用

#### Scenario: Bubblewrap executable is missing
- **WHEN** capability 配置选择 `bubblewrap` 且系统找不到 `bwrap`
- **THEN** worker 初始化 SHALL 失败并返回明确错误，而不是继续使用宿主执行
