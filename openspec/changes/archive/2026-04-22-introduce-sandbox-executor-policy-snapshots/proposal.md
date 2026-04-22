## Why

当前 `bubblewrap` sandbox 的核心问题不是“缺少更多 denylist”，而是权限持有位置不对：

- `read`、`write`、`edit` 这类文件工具仍由长生命周期 proxy 直接执行。
- proxy 需要持续负责拉起命令子进程，因此它不能过早把自己的进程创建能力收得太死。
- 一旦 proxy 自己持有整个 workspace 的稳定读写能力，就很难按 agent / 任务动态拆分文件写权限。

这会直接卡住多 agent 协作场景：上层即使已经决定“哪个 agent 可以写哪些路径”，底层也无法把这些授权变成真正的运行时隔离边界。对这个问题，正确方向不是热更新运行中进程的 `Landlock` / `seccomp`，而是让 proxy 作为控制面，为每次请求或每个 session 生成一份新的策略快照，再派生受限 executor 去执行实际动作。

## What Changes

- 引入“策略快照驱动的 sandbox executor”模型：proxy 维护动态授权源，并在每次文件/命令请求进入时生成一份不可变的 executor 策略快照。
- 将 `internal-sandbox proxy` 收敛为控制面：负责 JSON-RPC 协议、路径解析、请求校验、策略快照生成和 executor 调度，而不再直接执行 workspace 文件读写或最终用户命令。
- 把 `read`、`write`、`edit` 下沉到 one-shot file executor；把一次性命令下沉到 one-shot command executor；把交互式/后台命令下沉到 session executor。
- 明确 executor 的 staged enforcement：executor 先读取策略快照，再安装动态 Landlock；最终命令子进程在 `exec` 前再安装更严格的 seccomp，而不是要求 executor 自己先装会阻断 `fork/exec` 的最终 seccomp。
- 明确动态授权的生效边界：proxy 对授权源的更新只影响后续新起的 executor / session，而不会热更新已经运行中的 executor。
- 保持 `bubblewrap` 作为 namespace / mount 视图的基础层，不用动态 Landlock 替代 `bwrap` 本身。

## Capabilities

### New Capabilities

- `sandbox-executor-policy-snapshot`: 定义 proxy 生成 executor 策略快照、通过一次性 IPC 下发、并在 executor / 最终命令子进程中分阶段安装动态约束的语义。

### Modified Capabilities

- `sandbox-jsonrpc-proxy`: 将 proxy 从“直接执行文件与命令动作”的实现改为“控制面调度 executor”的语义。
- `sandbox-tool-routing`: 让 `read`、`write`、`edit`、`exec_command`、`write_stdin`、`list_unread_command_tasks` 统一路由到 executor，而不是继续由 proxy 直接碰 workspace 或直接持有最终命令进程。
- `sandbox-runtime`: 将 Bubblewrap runtime 从“单个长生命周期 proxy 持有主要权限”改为“长生命周期 proxy + 按请求/按 session 派生的受限 executor”。

## Impact

- Affected systems: `src/agent/sandbox.rs`、`src/cli.rs`、`src/agent/tool/read.rs`、`src/agent/tool/write.rs`、`src/agent/tool/edit.rs`、`src/agent/tool/command/process.rs`、`src/agent/tool/command/session.rs`
- Runtime impact: sandbox 将引入 file executor、one-shot command executor 和 session executor 三类执行路径；proxy 不再是 workspace 文件 I/O 的实际执行者。
- Security impact: 动态文件权限将从“proxy 持有固定宽权限”切换为“executor 按快照持有最小权限”；最终命令进程会在更靠近 `exec` 的位置安装最终 seccomp。
- Operational impact: 上层授权系统可以在两次请求之间调整路径读写范围，而无需热更新运行中进程；后台 session 若需要新权限，必须由上层显式重建 session。
- Testing impact: 需要补齐文件 executor、session executor、策略快照 IPC、Landlock 动态路径限制、最终 seccomp 安装和 fd 继承收口的测试。
