## Why

当前 `bash` 工具只支持一次性 shell 调用，既不能把长任务挂到后台，也不能在同一个线程里持续与 PTY/TUI 进程交互。继续在现有 schema 上零散补参数，会把“启动命令”“续写 stdin”“轮询输出”“查询状态”混成一个不稳定接口，也无法为后续更复杂的工具运行时提供统一抽象。

现在需要把这块能力正式提升为“线程内命令会话工具”，先把后台任务、交互式命令和任务状态查询这条链路跑通，再决定是否进一步扩展到更通用的 sidecar 或持久化执行模型。

## What Changes

- 新增一组始终可见的命令会话工具：`exec_command`、`write_stdin`、`list_unread_command_tasks`，把启动、交互、轮询和“列出仍有未读输出的命令 session”拆成稳定动作。
- 将当前 `bash` 收敛为兼容入口：保留现有一次性调用语义，但内部复用新的命令会话运行时，而不是继续作为唯一 shell 能力入口演进。
- 新增线程级命令会话运行时，支持 `workdir`、`shell`、`tty`、`yield_time_ms`、`max_output_tokens`、增量输出 chunk、任务状态和线程隔离。
- 为后台任务补齐运行时内存任务目录与状态导出能力，其中 `list_unread_command_tasks` 重点面向模型列出仍有未读输出的命令 session；完整的运行态 / 退出态与未读状态视图则保留给运行时导出接口，供未来 web/status 接口使用。
- 统一模型可见的工具输出为稳定纯文本摘要，明确展示当前命令、chunk、wall time、进程是否仍在运行以及本次输出，并把 `wall_time_seconds` 明确定义为“本次工具调用的墙钟耗时”；`list_unread_command_tasks` 的结构化返回也补齐 `wall_time_seconds`，方便调用方观测当前这次查询本身花了多久。
- 在 `resources/` 下增加一个可交互的 TUI 验证程序，并补齐自动化测试，覆盖交互成功、未结束轮询和自然退出等场景。

## Capabilities

### New Capabilities
- `command-session-tools`: 在线程内提供可后台运行、可交互、可列出运行中任务、可导出状态的命令会话工具能力，并用统一运行时承接现有一次性 `bash` 行为。

### Modified Capabilities

## Impact

- Affected code: `src/agent/tool/mod.rs`、当前 `src/agent/tool/shell.rs`、可能新增的 `src/agent/tool/command/` 模块，以及对应测试和 `resources/` 验证程序。
- API impact: always-visible builtin tools 将新增 `exec_command`、`write_stdin`、`list_unread_command_tasks`；`bash` 保留但进入兼容层定位。
- Runtime impact: `ToolRegistry` 需要管理线程级命令会话运行时、长生命周期子进程/PTY 资源，以及可导出的内存任务状态目录。
- Verification impact: 需要新增单元测试、集成测试或 `#[ignore]` smoke test，以及一个供工具联调用的 TUI fixture。
- Dependency impact: Unix 平台可能需要引入 PTY 相关依赖；非 PTY 路径继续复用现有 shell 启动能力。
