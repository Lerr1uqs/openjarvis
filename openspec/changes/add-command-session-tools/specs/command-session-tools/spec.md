## ADDED Requirements

### Requirement: 系统 SHALL 暴露命令会话工具族
系统 SHALL 在 always-visible builtin tool 集合中暴露 `exec_command`、`write_stdin`、`list_unread_command_tasks`，用于承接后台任务和交互式命令会话能力，而不是继续只提供一个一次性的 `bash` 接口。

#### Scenario: builtin tool 列表包含命令会话工具
- **WHEN** `ToolRegistry` 完成 builtin tools 注册
- **THEN** 当前可见工具列表中包含 `exec_command`、`write_stdin`、`list_unread_command_tasks`

### Requirement: 系统 SHALL 在迁移阶段保留 `bash` 兼容入口
系统 SHALL 在迁移阶段继续暴露 `bash`，但其定位 SHALL 是一次性命令执行兼容层，而不是后台任务主入口。`bash` SHALL 继续接受现有 `command` / `timeout(_ms)` 参数，并在内部复用命令会话运行时，同时保持“超时即返回错误，不暴露 session continuation”的旧语义。

#### Scenario: 兼容 `bash` 仍保持一次性超时语义
- **WHEN** 调用方继续使用 `bash` 执行一个在超时时间内未退出的命令
- **THEN** 工具返回超时错误
- **THEN** 返回结果中不会暴露新的会话续写句柄给旧调用方

### Requirement: `exec_command` SHALL 同时支持一次性执行与后台会话启动
`exec_command` SHALL 接受 `cmd`，并支持可选的 `workdir`、`shell`、`tty`、`yield_time_ms`、`max_output_tokens` 参数。系统 SHALL 在 `yield_time_ms` 窗口内等待命令输出或退出；若命令已结束，则直接返回本次输出和退出码；若命令仍在运行，则 SHALL 返回当前输出 chunk 与可继续交互的 `session_id`。当 `tty=true` 且平台不支持 PTY 时，系统 SHALL 显式失败，而不是静默退回普通 pipe。

#### Scenario: 短命令在等待窗口内直接完成
- **WHEN** `exec_command` 启动一个会在 `yield_time_ms` 内退出的命令
- **THEN** 返回结果中包含该次执行的输出和退出码
- **THEN** 调用方不需要再调用 `write_stdin` 才能拿到终态结果

#### Scenario: 长命令进入后台并返回会话句柄
- **WHEN** `exec_command` 启动一个在 `yield_time_ms` 内尚未结束的命令
- **THEN** 返回结果中包含 `session_id`
- **THEN** 返回结果中的 `output` 只包含当前已产生的增量输出 chunk

### Requirement: `exec_command` 与 `write_stdin` SHALL 返回稳定纯文本执行摘要
`exec_command` 与 `write_stdin` 的模型可见结果 SHALL 使用稳定纯文本摘要，而不是要求模型解析 JSON。摘要 SHALL 至少按固定顺序包含 `Command:`、`Chunk ID:`、`Wall time:`、进程状态行、`Original token count:` 和 `Output:`。当进程仍在运行时，状态行 SHALL 使用 `Process running with session ID <id>`；当进程已经退出时，状态行 SHALL 使用 `Process exited with code <code>`。如果该次返回时进程已经退出，且本次可返回的增量输出为空，摘要中的 `Output:` 行 SHALL 显式写成 `Output: NULL (当前程序已结束，缓冲区读取完毕)`，而不是只留下空白。与此同时，结构化返回值 SHALL 包含 `wall_time_seconds` 与 `exit_code` 字段，其中 `wall_time_seconds` SHALL 表达当前这一次工具调用实际花掉的墙钟时间，而不是 session 自启动以来的累计时间；`exit_code = null` SHALL 表示目标进程在当前调用返回时尚未退出。

#### Scenario: 运行中的会话返回 running 摘要
- **WHEN** `exec_command` 或 `write_stdin` 返回时目标进程仍在运行
- **THEN** 纯文本摘要中包含 `Process running with session ID <id>`
- **THEN** 模型可以直接从摘要文本判断该任务尚未结束

#### Scenario: 已退出的会话返回 exited 摘要
- **WHEN** `exec_command` 或 `write_stdin` 返回时目标进程已经退出
- **THEN** 纯文本摘要中包含 `Process exited with code <code>`
- **THEN** 模型可以直接从摘要文本判断该任务已经结束

#### Scenario: 已退出且无输出时显式标记输出已读空
- **WHEN** `exec_command` 或 `write_stdin` 返回时目标进程已经退出，且该次没有任何可返回的增量输出
- **THEN** 纯文本摘要中的 `Output:` 行使用 `Output: NULL (当前程序已结束，缓冲区读取完毕)`
- **THEN** 调用方不需要再把空白输出误判成“还可以继续读取”

### Requirement: `write_stdin` SHALL 续写或轮询既有命令会话
`write_stdin` SHALL 通过 `session_id` 访问已有命令会话，并与 `exec_command` 共享同一响应 schema。`chars` 为空字符串时，工具 SHALL 仅执行轮询而不写入 stdin；`chars` 非空时，工具 SHALL 先写入 stdin，再等待新的输出或进程退出。系统 SHALL 只返回该 session 自上一次成功读取以来的新输出 chunk，并通过单调递增的 `chunk_id` 标识该次增量结果。

#### Scenario: 空写请求只轮询后台输出
- **WHEN** 调用方使用 `write_stdin` 并传入 `chars = \"\"`
- **THEN** 工具不会向目标进程写入新内容
- **THEN** 工具仍会返回该 session 自上一次读取以来新增的输出或终态结果

#### Scenario: 交互式输入驱动 TUI 进入下一步
- **WHEN** 调用方向一个仍在运行的交互式 session 写入新的输入内容
- **THEN** 目标进程收到对应 stdin 内容
- **THEN** 工具返回新的输出 chunk 或最终退出结果

### Requirement: 系统 SHALL 维护线程级命令任务目录，并让 `list_unread_command_tasks` 只列出存在未读输出的 session
系统 SHALL 按 internal thread 维护命令任务目录，使后台任务信息能够在同一线程的后续工具调用之间持续可见。`list_unread_command_tasks` SHALL 只返回当前线程仍然存在未读增量输出的命令 session，而不是把运行时内存中的全部任务都摊给模型，也不负责枚举“正在运行但当前没有新输出”的 silent session。每个任务条目 SHALL 至少包含 `session_id`、`command`、`exit_code` 和最近更新时间；其中 `exit_code = null` 表示该 session 在当前刷新视角下尚未观察到退出。该工具的结构化返回值 SHALL 同时包含 `tasks` 与 `wall_time_seconds`，其中 `wall_time_seconds` SHALL 表达当前这一次 list 调用自身花掉的墙钟时间，而不是任何 session 的累计运行时间。

#### Scenario: list 工具只返回还有未读输出的 session
- **WHEN** 同一线程先后启动多个命令，其中至少一个已经产生了模型尚未读取的新输出，而至少一个没有任何未读输出
- **THEN** `list_unread_command_tasks` 只返回存在未读输出的命令 session
- **THEN** 每个条目都包含对应的 `session_id`、命令摘要、`exit_code` 和最近更新时间

#### Scenario: silent running session 不出现在 unread 列表中
- **WHEN** 一个后台命令仍在运行，但自上一次成功读取以来尚未产生新的输出
- **THEN** `list_unread_command_tasks` 不返回该 session
- **THEN** 调用方如果仍持有该 `session_id`，仍可主动通过空写 `write_stdin` 继续轮询

#### Scenario: 其他线程不能访问当前线程的命令会话
- **WHEN** 另一个 internal thread 使用不属于自己的 `session_id` 调用 `write_stdin`
- **THEN** 工具调用显式失败
- **THEN** 该失败不会影响原线程中任务的继续运行

#### Scenario: 已退出但仍有尾部输出未读的 session 仍会出现在 unread 列表中
- **WHEN** 一个后台命令已经自然退出，但其最后输出 chunk 尚未被模型读取
- **THEN** `list_unread_command_tasks` 仍返回该 session
- **THEN** 后续一次空写 `write_stdin` 可以读取该尾部输出，并在返回值里看到非空 `exit_code`

#### Scenario: 当前线程没有未读输出时返回空列表
- **WHEN** 当前线程内所有命令 session 都不存在未读输出
- **THEN** `list_unread_command_tasks` 返回空列表
- **THEN** 调用方可以据此判断当前没有需要继续读取的命令输出

### Requirement: 系统 SHALL 提供可导出的运行时内存任务摘要
系统 SHALL 在命令会话运行时内维护一份可导出的只读任务摘要视图，供未来 web/status 页面复用。该摘要视图 SHALL 不暴露 live process handle，但 SHALL 至少包含 `thread_id`、`session_id`、`status`、`command`、`has_unread_output`、`exit_code`、最近更新时间等基础字段。该导出视图中的 `status` SHALL 至少覆盖 `Doing` 与 `Done`，而 `has_unread_output` SHALL 用于表达当前是否仍存在尚未被读取的增量输出。

#### Scenario: 运行时可以导出后台任务状态快照
- **WHEN** 程序内其他组件需要展示当前运行时的后台任务状态
- **THEN** 命令会话运行时可以导出不含 live process handle 的任务摘要视图
- **THEN** 该视图可以同时表达 Doing / Done 与是否仍有未读输出

### Requirement: 系统 SHALL 提供可自动化验证的命令会话示例程序
系统 SHALL 在 `resources/` 下提供一个专门用于验证命令会话工具的交互式程序，使测试能够通过 `exec_command` 和 `write_stdin` 驱动真实 TUI/交互流程，而不是只依赖 mock。该示例程序 SHALL 至少覆盖“多轮输入后返回 `OK`”和“命令在中途保持未完成”两类验证场景。

#### Scenario: 工具驱动示例程序完成求和交互
- **WHEN** 自动化测试通过 `exec_command` 启动示例程序，并用多次 `write_stdin` 完成交互
- **THEN** 程序最终输出 `OK`
- **THEN** 工具链路证明后台命令、stdin 续写与输出轮询可以协同工作

#### Scenario: 示例程序保持未完成以验证轮询路径
- **WHEN** 自动化测试启动示例程序但暂不提交最终答案
- **THEN** 调用方可以通过空写 `write_stdin` 继续轮询输出
- **THEN** 当该程序产生新输出但尚未被读取时，`list_unread_command_tasks` 会返回对应 session
