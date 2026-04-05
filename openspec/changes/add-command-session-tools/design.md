## Context

当前仓库里的 `bash` 工具实现还是一次性命令执行：只接受 `command` 和 `timeout`，内部直接 `sh -lc` 或 `powershell -Command` 拉起子进程，等待退出后一次性返回 stdout/stderr。这个实现对“短命令”足够，但无法覆盖用户现在明确提出的两类场景：

- 长时间运行的后台任务，需要先启动、稍后再看结果
- 交互式 PTY/TUI 程序，需要通过多次 stdin 写入完成一轮完整对话

仓库里已经有可复用的线程级运行时模式，比如 browser toolset 会按 `thread_id` 持有独立 session，并在后续工具调用中复用。命令会话问题和它类似，但又有两个不同点：

- 命令能力是基础 builtin，而不是按需加载 toolset
- 用户希望它可以最终“代替现有 bash 工具”，但又不适合直接把旧接口硬改成一个巨大的多态 schema

因此这次 change 的关键不是再给 `bash` 多加几个参数，而是把“命令执行”正式提升为线程级会话运行时，同时保留兼容入口降低迁移风险。

## Goals / Non-Goals

**Goals:**
- 定义一组始终可见的命令会话工具，覆盖启动、续写 stdin、轮询输出和列出未读输出任务。
- 支持同一 internal thread 内的后台命令和交互式 PTY/TUI 命令。
- 为任务提供稳定的状态目录和查询接口，而不是只在单次工具调用里短暂存在。
- 让任务状态目录可以作为运行时内存快照被导出，供未来 web/status 页面复用。
- 让 `bash` 可以在首版中继续保持兼容语义，避免一次性打断现有调用链。
- 提供一个 `resources/` 下的可交互验证程序，用真实命令序列回归工具正确性。

**Non-Goals:**
- 本次不实现“进程重启后重新附着正在运行的子进程”这类跨进程恢复能力。
- 本次不把命令任务升级成分布式 job scheduler，也不做跨机器任务路由。
- 本次不引入新的审批/安全策略模型，命令权限仍沿用当前工具运行面。
- 本次不实现服务端主动推送输出给模型；输出仍通过工具轮询返回。
- 本次不修改 `model/**` 架构文档，只在 OpenSpec change 范围内定义需求与设计。

## Decisions

### 1. 新增命令会话工具族，`bash` 先保留为兼容包装层

首版工具面直接采用新的动作拆分：

- `exec_command`
- `write_stdin`
- `list_unread_command_tasks`

这样模型面对的是清晰的运行时动作，而不是把“启动命令”和“继续某个 session”都塞进一个 `bash` 参数对象里。与此同时，现有 `bash` 不立即删除，而是保留为兼容层：

- 仍接受当前 `command` / `timeout(_ms)` 参数
- 内部复用新的命令会话后端
- 仍保持“命令超时就报错，不暴露 session_id”的一次性语义

这样既能让新能力尽快落地，也不会立刻打断现有 prompt、测试和调用习惯。

Alternative considered:
- 直接把 `bash` 扩成一个同时支持启动、续写、查询的大接口。
  Rejected，因为 schema 会迅速变得混乱，模型也更难稳定选择正确动作。

Alternative considered:
- 直接移除 `bash`，只保留新工具。
  Rejected，因为当前仓库还有多处基础四工具假设，第一阶段先兼容更稳妥。

### 2. 实现落到新的 `src/agent/tool/command/` 模块，`shell.rs` 只保留兼容适配职责

后台任务和交互式命令已经不再只是“shell 一次性执行”。为了保持模块边界清晰，建议把实现拆到新的命令模块中，例如：

- `command/mod.rs`: 导出注册入口与公共类型
- `command/session.rs`: 线程级 session / task 目录
- `command/process.rs`: 子进程/PTY 启动与 IO 管理
- `command/output.rs`: 增量输出 chunk、截断与计数
- `command/tool.rs`: `exec_command` / `write_stdin` / 查询工具实现

原来的 `shell.rs` 则降级为兼容适配层，专门承接 legacy `bash`。

Alternative considered:
- 继续把所有新逻辑都塞在 `shell.rs`。
  Rejected，因为这会让 builtin shell 文件同时承担兼容层、后台任务、PTY 和查询 API，耦合过高。

### 3. 使用共享的 `CommandSessionManager` 按 `thread_id` 管理命令任务

命令会话运行时采用线程级隔离模型。`ToolRegistry` 在注册 always-visible handlers 时创建一个共享的 `CommandSessionManager`，并把它注入给所有命令相关工具 handler。该 manager 负责：

- 按 `thread_id` 维护任务目录
- 为每个任务分配 `session_id`
- 持有运行中的进程句柄、stdin writer、输出缓存和状态
- 在任务结束后保留摘要信息，供未来 web/status 导出接口读取
- 提供不暴露 live process handle 的只读快照视图

由于当前 builtin tools 没有独立的 runtime lifecycle hook，这次不额外发明新的全局生命周期框架，而是先用共享 manager 承接。首版运行面只提供“启动、交互、观察、列出未读输出任务”，不新增面向模型的进程终止工具；manager 只需要负责自然退出后的回收，以及完成态任务的有界保留或惰性清理。

Alternative considered:
- 先给 always-visible builtin tools 新增一套运行时生命周期注册机制。
  Rejected，因为这会扩大到整个工具子系统，超过本次 change 的必要范围。

Alternative considered:
- 同时新增模型可见的进程终止工具。
  Rejected，因为用户明确收窄了当前目标，模型只需要知道进程是否结束并继续交互；额外的 kill 控制面可以后续按管理接口再补。

### 4. `exec_command` 和 `write_stdin` 共享统一纯文本摘要格式，按“增量 chunk”返回输出

`exec_command` 与 `write_stdin` 的模型可见 `content` 使用同一套稳定纯文本包裹，至少包含：

- `chunk_id`
- `wall_time_seconds`
- `exit_code`
- `session_id`
- `original_token_count`
- `output`

其中：

- `exec_command` 负责创建命令并等待 `yield_time_ms`
- 如果命令在等待窗口内结束，则直接返回终态结果
- 如果命令仍在运行，则返回 `session_id` 和当前增量输出
- `write_stdin` 既可以写入字符，也可以在 `chars = \"\"` 时只做轮询
- `output` 只返回该 session 自上一次成功读取以来的新输出 chunk，避免重复膨胀
- `wall_time_seconds` 表达当前这一次工具调用实际花掉的墙钟时间，而不是 session 自启动以来的累计时间
- 运行中的摘要行使用 `Process running with session ID <id>`
- 已退出的摘要行使用 `Process exited with code <code>`
- 如果命令已退出且本次没有任何增量输出，`Output:` 行显式写成 `Output: NULL (当前程序已结束，缓冲区读取完毕)`，避免把空白误判成“还没读到结果”

`max_output_tokens` 按 best-effort 上限裁剪单次返回内容，并通过 `original_token_count` 暴露裁剪前规模。

Alternative considered:
- 单独再加一个 `poll_command_output` 工具。
  Rejected，因为空写轮询已经足够表达该语义，额外工具只会增加选择噪声。

### 5. `tty=true` 走 PTY 路径，`tty=false` 走普通 pipe；不支持的平台显式失败

为了让 `resources/` 下的 TUI 验证程序真正可测，首版必须支持 PTY 模式。设计上：

- `tty=true` 时使用 PTY 启动子进程，适配需要终端能力的程序
- `tty=false` 时继续使用普通 stdin/stdout/stderr pipe
- 如果当前平台暂不支持 PTY，实现必须显式报错，而不是偷偷退回 pipe 模式

这样可以避免“工具看起来成功，但 TUI 根本没按交互终端运行”的隐性错误。

Alternative considered:
- 所有命令都只走普通 pipe，不提供 PTY。
  Rejected，因为用户给出的验收用例本身就是交互式 TUI。

### 6. 首版只承诺“运行时内存持久与状态导出”，不承诺跨进程恢复

用户明确提到要预留“后台任务相关信息”和“在内存中的持久化能力”。本次将其解释为：

- 同一个 internal thread 在后续多次工具调用之间，能继续看到既有任务和状态
- 完成态任务在运行时内存中保留摘要，直到被清理
- 运行中的 session 可以被 `write_stdin` 继续访问
- `list_unread_command_tasks` 只负责列出仍存在未读输出的命令 session
- 命令运行时可以导出只读任务摘要，供未来 web/status 页面直接展示 Doing/Done、是否有未读输出以及退出码
- `list_unread_command_tasks` 的结构化返回补齐 `tasks` 与 `wall_time_seconds`，其中 `wall_time_seconds` 只反映当前这一次查询动作本身的墙钟耗时

但本次不承诺：

- 进程重启后重新附着旧任务
- 把 live PTY/进程句柄持久化到 store

如果后续需要做跨重启恢复，可以在相同 task record 模型上继续扩展 durable snapshot。

Alternative considered:
- 这一版就把 live command session 做成可持久化、可恢复的运行时。
  Rejected，因为复杂度和不确定性都明显过高，会拖慢核心工具契约的落地。

### 7. 验证入口采用 `resources/` TUI fixture + 自动化工具驱动测试

按用户建议，在 `resources/` 下增加一个专门用于验证的交互式程序。推荐它至少覆盖两类流程：

- 成功路径：输入 A 拿到数字，输入 B 再拿到数字，把两者相加后回填，最终返回 `OK`
- 过程路径：故意保持未完成，验证后台 session、空写轮询和自然退出后的状态变化

自动化测试直接通过新工具驱动该程序，而不是只对内部函数做 mock，从而确保真实的 PTY / stdin / 输出 chunk 链路可回归。

Alternative considered:
- 只写单元测试，不提供真实交互 fixture。
  Rejected，因为这无法证明后台任务和交互式会话工具真正可用。

## Risks / Trade-offs

- [always-visible 命令会话运行时可能积累长时间运行任务或完成态垃圾] -> 首版先通过任务列表和退出信息让运行态可观测，并为完成态任务增加有界保留或惰性清理；如果后续确实需要人工终止，再单独增加管理控制面。
- [PTY 行为存在平台差异，测试结果可能不一致] -> 首版重点验证 Unix 路径，其他平台在 `tty=true` 时显式失败。
- [`max_output_tokens` 是 best-effort，而不是精确 tokenizer 语义] -> 在响应里返回 `original_token_count` 和 `chunk_id`，让调用侧知道发生过裁剪。
- [兼容保留 `bash` 会在一段时间内形成双入口] -> 明确 `bash` 只是兼容层，并让新测试和新文档优先使用 `exec_command` 系列。

## Migration Plan

1. 新增 `src/agent/tool/command/` 模块和共享 `CommandSessionManager`，但先不删除现有 `bash` 入口。
2. 注册新的命令会话工具，先让新接口可用，再把 `bash` 改成复用新后端的一次性包装层。
3. 增加任务目录、状态查询和可导出的内存快照，补齐线程隔离与错误路径测试。
4. 在 `resources/` 下加入 TUI 验证程序，并增加工具级真实链路测试。
5. 待调用面迁移稳定后，再决定是否把 `bash` 从兼容层彻底移除。

Rollback strategy:
- 回滚时可以先移除新命令会话工具注册并恢复原始 `bash` 实现；由于首版保留兼容入口，回退路径相对直接。

## Open Questions

- `bash` 是在本 change 内标记 deprecated 即可，还是要顺手清理 prompt / 文档里对“基础四工具”的直接表述。
- 完成态任务在内存中的默认保留策略应该是按数量上限、按时间 TTL，还是两者并用。
- 运行时导出的任务摘要是否要在同一 change 内写入线程快照元数据，还是先只做运行时内存目录。
