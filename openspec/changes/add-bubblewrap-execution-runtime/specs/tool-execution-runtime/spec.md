## ADDED Requirements

### Requirement: 工具执行层 SHALL 支持本地与沙箱两种运行环境
系统 SHALL 在工具子系统中引入统一执行层，并允许运行时在 `Local` 与 `Sandbox` 两种执行环境之间选择，而不要求 tool handler 自己维护宿主/沙箱双分支逻辑。

#### Scenario: 本地环境保持当前宿主执行语义
- **WHEN** 工具运行时配置选择 `Local`
- **THEN** 文件访问和子进程启动 SHALL 直接在宿主环境中执行，并保持现有工具调用语义

#### Scenario: 沙箱环境通过统一入口提供执行能力
- **WHEN** 工具运行时配置选择 `Sandbox`
- **THEN** 工具子系统 SHALL 通过统一执行层处理文件访问和子进程动作，而不是让 tool handler 直接访问宿主文件系统或直接启动宿主子进程

### Requirement: 工具子系统 SHALL 通过执行层承接文件与进程副作用
系统 SHALL 让所有会直接读写文件或启动工具自有子进程的工具能力通过执行层完成副作用，包括 builtin 文件工具、shell 工具、memory 工具，以及工具自有 sidecar 或 stdio server 生命周期。

#### Scenario: builtin 文件工具通过执行层完成文件访问
- **WHEN** `read`、`write` 或 `edit` 工具处理一次请求
- **THEN** 其文件读取、写入和替换动作 SHALL 通过执行层完成，而不是在 tool handler 中直接操作宿主文件系统

#### Scenario: tool-owned 子进程通过执行层启动
- **WHEN** `bash`、browser sidecar 或 stdio MCP server 需要启动子进程
- **THEN** 子进程 SHALL 由执行层启动和回收，以便本地与沙箱环境共用同一套上层工具逻辑

### Requirement: 沙箱执行环境 SHALL 由 Bubblewrap helper 提供实现
在 Linux 上，当配置选择 `Sandbox` 时，系统 SHALL 使用 Bubblewrap 启动一个长期存活的内部 helper 进程，并通过结构化本地协议把执行请求转发给该 helper。

#### Scenario: 启动沙箱 helper
- **WHEN** agent runtime 初始化一个 Bubblewrap 沙箱执行环境
- **THEN** 系统 SHALL 通过 `bwrap` 启动内部 helper 进程，并在后续工具调用中复用该 helper，而不是为每次调用都创建一次全新沙箱

#### Scenario: helper 执行结构化请求
- **WHEN** 工具执行层请求一次文件操作或进程操作
- **THEN** 沙箱 helper SHALL 返回结构化成功或失败结果，使上层工具可以继续使用统一的工具结果包装

### Requirement: Bubblewrap 沙箱 SHALL 使用显式挂载与环境策略
系统 SHALL 通过显式 bind mount、工作区路径映射和环境变量清理来定义沙箱边界，而不是把宿主环境完整暴露给工具。

#### Scenario: 工作区路径映射到沙箱路径
- **WHEN** 工具请求访问当前工作区内的路径
- **THEN** 执行层 SHALL 将该路径映射到沙箱内固定工作区挂载点，并只在允许的挂载范围内完成访问

#### Scenario: 非授权宿主路径不会被隐式暴露
- **WHEN** 工具请求访问未被挂载或未被策略允许的宿主路径
- **THEN** 沙箱执行 SHALL 返回显式失败，而不是自动退回宿主本地执行

### Requirement: 配置错误或环境不支持时 SHALL 显式失败
当用户配置 `Sandbox` 运行环境但 Bubblewrap 不可用、平台不支持或 helper 无法启动时，系统 SHALL 返回明确初始化错误，而不是静默回退到 `Local`。

#### Scenario: 缺少 Bubblewrap 可执行文件
- **WHEN** 运行配置选择 `Sandbox` 且系统无法找到可用的 `bwrap`
- **THEN** agent runtime 初始化 SHALL 失败并返回明确错误，指出沙箱后端不可用

#### Scenario: helper 启动失败
- **WHEN** Bubblewrap 进程或内部 helper 启动失败
- **THEN** 系统 SHALL 返回明确错误并阻止把该执行环境继续提供给工具子系统
