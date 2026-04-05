## ADDED Requirements

### Requirement: 系统 SHALL 通过注册分发层执行顶层 CLI subcommand

系统 SHALL 提供一个顶层 CLI subcommand 注册分发层。`openjarvis` 在解析出顶层 subcommand 后，SHALL 通过该分发层把执行请求交给对应的注册 executor，而不是在 `main` 中为每个子命令持续追加手写分支。

#### Scenario: 已注册 subcommand 通过对应 executor 执行

- **WHEN** 用户执行 `openjarvis skill install acpx`
- **THEN** 系统会把 `skill` 顶层 subcommand 分发给已注册的 `skill` executor
- **THEN** `main` 自身不直接承载 `skill install` 的具体业务逻辑

### Requirement: 系统 SHALL 让现有内部 helper 也走统一注册分发

系统 SHALL 让现有顶层内部 helper 子命令也接入同一套注册分发层，至少包括 `internal-mcp` 与 `internal-browser`。

#### Scenario: 内部 helper 通过注册 executor 执行

- **WHEN** 用户执行 `openjarvis internal-browser smoke --url https://example.com`
- **THEN** 系统会把 `internal-browser` 顶层 subcommand 分发给对应的已注册 executor
- **THEN** executor 负责调用对应 helper 逻辑并返回结果

### Requirement: 系统 SHALL 在未命中顶层 subcommand 时继续正常进入主程序运行面

系统 SHALL 在命令行未提供顶层 subcommand 时继续执行主程序默认启动流程，而不是误进入 CLI executor 分发路径。

#### Scenario: 无 subcommand 时继续启动主服务

- **WHEN** 用户执行 `openjarvis` 或只传入全局参数而未指定顶层 subcommand
- **THEN** 系统不会调用任何 CLI executor
- **THEN** 系统继续执行配置加载、运行时装配和 router 启动流程
