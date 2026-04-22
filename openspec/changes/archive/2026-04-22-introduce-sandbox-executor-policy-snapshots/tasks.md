## 1. Proxy 与策略快照契约

- [x] 1.1 定义 executor 策略快照结构，覆盖路径读写集合、动作类型、session 标识和 seccomp tier
- [x] 1.2 定义 proxy 到 executor 的一次性 IPC 传输契约，并明确策略快照读取完成后的 fd 关闭与不继承要求

## 2. Proxy 控制面重构

- [x] 2.1 重构 `internal-sandbox proxy`，移除其对 workspace 的直接 `read/write/edit` 执行路径，改为生成策略快照并派生 file executor
- [x] 2.2 重构命令路由，使 proxy 不再直接持有最终用户命令进程，而是启动 one-shot command executor 或 session executor

## 3. Executor 执行模型

- [x] 3.1 实现 one-shot file executor，在读取策略快照后安装动态 Landlock 并完成单次文件动作
- [x] 3.2 实现 one-shot command executor 与 session executor，前者负责单次命令，后者负责 PTY/pipe 监督、`write_stdin` 和 `list_unread_command_tasks`
- [x] 3.3 在命令 executor 中拆分 Landlock 与最终 seccomp 的安装阶段，确保最终 seccomp 仅在最终命令 `exec` 前安装

## 4. 工具路由与回归测试

- [x] 4.1 更新 `read`、`write`、`edit`、`exec_command`、`write_stdin`、`list_unread_command_tasks` 的 sandbox 路由，使其统一走 executor
- [x] 4.2 增加集成测试，覆盖动态授权更新只影响未来 executor、后台 session 需要重建才能获得新权限、未授权路径直接失败和策略 IPC 不泄漏到最终命令
