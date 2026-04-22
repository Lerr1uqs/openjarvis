## Context

当前 sandbox 架构已经具备三块基础设施：

- `bubblewrap` 提供 namespace / mount 视图
- `internal-sandbox proxy` 提供 JSON-RPC 控制通道
- `exec_command` 已经有一个 child helper，能够在最终命令执行前安装固定 profile

但核心工具路径仍然割裂：

- 文件工具由 proxy 自己直接读写 workspace
- 命令工具虽然有 helper，但 proxy 仍然是长生命周期的高权限持有者
- proxy 一旦先装了过严的 seccomp，就可能无法继续拉起后续 child；如果不装，又无法把“最终命令之前”的窗口收得足够小

这使得“多 agent 动态文件权限”很难落实为真正的内核边界。因为只要 proxy 本身仍然持有稳定的 workspace 读写权限，上层再精细的授权决策也只能停留在应用层检查，而不能成为 executor 的硬约束。

## Goals / Non-Goals

**Goals:**

- 让 proxy 只承担控制面职责，不再直接执行 workspace 文件读写或最终用户命令。
- 让每次文件请求、一次性命令请求、交互式/后台命令 session 都对应一个按快照授权的 executor。
- 让动态授权通过“生成新快照并启动新 executor”生效，而不是热更新运行中进程。
- 保留 `bubblewrap` 的 namespace / mount 隔离，并在 executor 中动态安装 Landlock。
- 将最终 seccomp 的安装位置收敛到更靠近最终命令 `exec` 的阶段，避免过早阻断 executor 自己的 `fork/exec`。
- 保持当前工具的路径语义、session 语义和错误透传语义。

**Non-Goals:**

- 本次不支持热更新已经运行中的 executor 或 session executor。
- 本次不要求 sandbox 层解决“两个 agent 同时被授权写同一路径”时的调度冲突；这仍由上层授权/调度系统负责。
- 本次不把 seccomp 扩展为任意用户自定义 DSL；首版仍以有限的内置 profile / tier 为主。
- 本次不移除 `bubblewrap`，也不尝试只靠 Landlock/seccomp 替代 namespace / mount 隔离。
- 本次不把 proxy 变成通用文件系统服务；它只负责校验、调度和结果回传。

## Decisions

### 1. Proxy 收敛为控制面，不再直接执行 workspace I/O 或最终用户命令

新的 proxy 职责只有四类：

- 接收和返回 JSON-RPC 请求
- 保持当前路径语义与错误语义
- 根据上层授权状态生成 executor 策略快照
- 启动并管理 executor

proxy 不再直接：

- 对 workspace 执行 `read/write/edit`
- 直接持有最终 bash / shell 进程
- 让最终命令继承自己的宽权限窗口

这样做的直接结果是：workspace 读写权限从“长生命周期 proxy”迁移到“短生命周期或 session 级 executor”。

Alternative considered:

- 继续让 proxy 直接执行文件工具，只把命令工具下沉到 child helper。
  Rejected，因为这样仍然无法把多 agent 的文件写权限做成内核级动态授权。

### 2. 动态授权通过“策略快照”实现，而不是热更新运行中进程

proxy 维护一份可变的授权源。它可以来自内存态、内部文件或上层显式下发的授权结果，但对 executor 生效的永远是一份 spawn 时冻结的快照。

快照至少需要表达：

- 本次 executor 可读路径集合
- 本次 executor 可写路径集合
- 是否允许显式 `/tmp`
- 目标动作类型：`read` / `write` / `edit` / `command-once` / `command-session`
- 选用的 seccomp tier
- 对 session executor 而言的 session 标识和后续续写边界

策略更新的语义明确为：

- 影响未来新启动的 executor
- 不影响已经运行中的 executor
- 若后台 session 需要新权限，上层必须显式结束并重建 session

Alternative considered:

- 让 proxy 在 executor 运行中途推送新的 Landlock/seccomp 规则。
  Rejected，因为 `Landlock` 和 `seccomp` 都是单向收紧模型，不适合作为运行中热更新机制。

### 3. 策略快照通过一次性 IPC 下发，最终命令子进程不继承策略源

executor 不直接读取 proxy 的策略文件或长生命周期授权源。更合理的方式是：

- proxy 在本地生成一份快照
- 通过一次性 IPC 把它交给 executor
- executor 读取完成后立即关闭这条 IPC
- executor 安装自身约束后，再派生最终命令子进程
- 最终命令子进程不再继承策略源 fd、proxy 控制通道或其他不必要 fd

这样可以避免两类问题：

- 最终命令仍能回头读取 proxy 的授权源
- 先装约束后再发现自己还持有不该有的 fd capability

在传输手段上，`stdin`/pipe、专用 Unix socket 或 `memfd` 都可以；设计层只要求“单次传递、可关闭、默认不继承”。

Alternative considered:

- 让 executor 直接按路径去读取共享策略文件。
  Rejected，因为这会扩大 executor 在 setup 期需要的读权限，也更容易把策略源误暴露给最终命令子进程。

### 4. 文件工具统一走 one-shot file executor

`read`、`write`、`edit` 的共同点是：

- 生命周期短
- 不需要长期保留 PTY/pipe
- 对路径权限最敏感

因此这三类工具统一走 one-shot file executor：

1. proxy 解析请求路径并生成快照
2. 启动 executor
3. executor 读取快照
4. executor 设置 `no_new_privs`
5. executor 安装动态 Landlock
6. executor 执行单次文件动作并退出

`edit` 继续保持“先读、应用精确替换、再写回”的现有语义，但这个读写闭环都发生在同一个 file executor 内，而不是由 proxy 自己完成。

Alternative considered:

- 为 `read`、`write`、`edit` 分别设计不同 helper。
  Rejected，因为它们共享同一套路径授权模型和 setup 生命周期，没有必要拆成多套 executor 入口。

### 5. 命令工具分为 one-shot command executor 与 session executor

命令执行需要分成两类：

- 一次性命令：执行、收集结果、退出
- 交互式/后台命令：需要保留 PTY/pipe、支持 `write_stdin` 和 `list_unread_command_tasks`

因此命令链路分成两种 executor：

- one-shot command executor：负责单次命令启动与结果收集
- session executor：负责会话生命周期、PTY/pipe 监督和后续续写

proxy 仍保留线程/session 视角的路由和结果聚合，但真正持有命令进程句柄、PTY 或 pipe 的对象变成 session executor，而不是 proxy 本身。

Alternative considered:

- 让所有命令都走 one-shot executor，再由 proxy 自己兜底保存 PTY/pipe。
  Rejected，因为这会让 proxy 再次回到“长期持有命令执行能力”的状态，和重构目标相冲突。

### 6. Landlock 与 seccomp 分阶段安装

新的命令执行路径明确分两段：

- executor 先读取快照并安装动态 Landlock
- 最终命令子进程在 `exec` 前安装最终 seccomp

这样拆分的原因很直接：

- executor 自己还要完成 `fork/exec`、PTY/pipe 建立、session 监督等动作
- 如果把最终 seccomp 过早装在 executor 自己身上，就容易把这条启动路径先打死
- 但 Landlock 主要收的是文件对象边界，适合在 executor 阶段先安装

对于纯文件 executor，可以只安装 Landlock，seccomp 仍按实现阶段决定是否补一层较宽的 setup profile；设计上不强制纯文件工具也必须带最终 seccomp。

Alternative considered:

- 一进入 executor 就安装最终 seccomp。
  Rejected，因为它会把 executor 的启动职责与最终命令的最小 syscall 面耦合到一起。

### 7. 路径竞争由上层授权/调度解决，sandbox 只保证越权失败

动态 Landlock 解决的是：

- 谁可以读哪些路径
- 谁可以写哪些路径
- 未授权路径一律失败

它不解决：

- 两个 agent 都被授权写同一路径时，谁先写、谁该让路
- 同一文件上的业务级冲突仲裁

这些仍然属于上层调度与授权系统的职责。sandbox 需要保证的边界只是：超出当前快照授权范围的访问必须直接失败，而不是“还能因为 proxy 的宽权限侥幸成功”。

## Risks / Trade-offs

- 进程模型比当前更复杂：文件工具、一次性命令和后台 session 都会引入新的 executor 生命周期。
- executor 数量会增加，短命进程开销会比“proxy 直接做文件 I/O”更高。
- fd 继承 hygiene 要求更严格；如果 proxy 控制通道、目录 fd 或策略 IPC 泄漏到最终命令，Landlock 也补不回来。
- session executor 的生命周期和回收需要更清晰的观测与日志，否则调试交互式命令会更困难。
- seccomp 先保持内置 tier，会让动态授权主要集中在 Landlock 路径规则；这比“全动态 seccomp”保守，但更可实现。

## Migration Plan

1. 定义 executor 策略快照结构、一次性 IPC 传输方式和 executor kind。
2. 把 `read`、`write`、`edit` 从 proxy 直接 I/O 迁移到 one-shot file executor。
3. 把现有 command helper 泛化为 one-shot command executor 与 session executor。
4. 将动态 Landlock 安装移入 executor，将最终 seccomp 安装收敛到最终命令 `exec` 前。
5. 清理 proxy 对 workspace 直接 I/O 和最终命令进程句柄的持有。
6. 补齐集成测试，覆盖策略快照更新只影响未来 executor、session 重建、fd 不继承和 Landlock 越权失败。

Rollback strategy:

- 如果需要回退，可以恢复 proxy 直接文件 I/O 和现有 command helper 路径，同时移除 executor 策略快照与 session executor；`bubblewrap` 的基础 runtime 仍可保留。

## Open Questions

- 策略快照的稳定序列化格式最终是否直接复用现有 JSON 协议，还是单独定义更偏内部 helper 的紧凑结构。
- session executor 是否需要独立的内部 CLI 子命令，还是可以复用同一个 `internal-sandbox executor` 入口并通过 kind 区分。
- 对纯文件 executor，是否还要额外补一个较宽的 setup seccomp tier，还是首版仅用 Landlock 即可。
