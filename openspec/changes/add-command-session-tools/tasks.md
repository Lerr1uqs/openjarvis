## 1. 命令会话运行时骨架

- [x] 1.1 在 `src/agent/tool/command/` 下拆分 session、process、output、tool 等模块，并把现有 `shell.rs` 收敛为兼容适配层或迁移入口
- [x] 1.2 在 builtin tool 注册路径中注入共享 `CommandSessionManager`，完成 `exec_command`、`write_stdin`、`list_unread_command_tasks` 的注册

## 2. 命令执行与后台交互

- [x] 2.1 实现 `exec_command` 的 shell/workdir/tty/yield/output 截断语义，覆盖一次性完成和后台返回 `session_id` 两条路径
- [x] 2.2 实现 `write_stdin` 的 stdin 写入、空写轮询、增量输出 chunk、退出码回收和状态迁移
- [x] 2.3 实现稳定纯文本结果格式，统一展示命令、chunk、wall time、运行中 session 或退出码以及当前输出，并让 `wall_time_seconds` 表达本次调用耗时
- [x] 2.4 实现线程级内存任务目录与 `list_unread_command_tasks`，让工具只列出仍有未读输出的 session，同时提供可导出的只读状态快照覆盖 Doing/Done、未读状态、退出码和跨线程隔离

## 3. 验证与回归

- [x] 3.1 在 `tests/agent/tool/` 下补齐单元测试，覆盖正常命令、失败、后台轮询、未知 session、线程隔离、内存状态导出和兼容 `bash`
- [x] 3.2 在 `resources/` 下新增交互式 TUI 验证程序，支持“两次输入数字再回填求和”的成功路径
- [x] 3.3 增加集成测试或 `#[ignore]` smoke test，使用 `exec_command + write_stdin` 驱动 TUI，并覆盖未结束轮询与自然退出场景
