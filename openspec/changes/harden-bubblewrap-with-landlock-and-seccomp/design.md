## Context

当前仓库已经有一条可工作的 `bubblewrap -> internal-sandbox proxy -> JSON-RPC -> core tool / command session` 路径，但它的主要安全边界仍来自：

- `bubblewrap` 提供的 namespace / mount 视图
- 宿主侧 `SandboxPathPolicy` 的路径规范化与拒绝逻辑
- proxy 内对文件和命令操作的结构化转发

这意味着现有 sandbox 更像“可观察的运行时边界”，而不是完整的 kernel-level capability enforcement：

- proxy 进程本身还没有进入 loop 前的 `no_new_privs + Landlock` 收口
- `exec_command` 派生的子进程仍共享 proxy 的宽权限窗口
- `bubblewrap`、Landlock、Seccomp 三层策略没有被编译成一个统一 enforcement plan
- 配置层也还没有表达“baseline proxy”和“command child”两级 profile 的能力

本次设计的约束是：

- 不替换现有 `bubblewrap` backend，而是在其上增量叠加内核收口层
- 不推翻现有 JSON-RPC proxy 结构，而是让 proxy 成为 enforcement 的第一落点
- 不把 policy 写死在代码里，而是继续从 `config/capabilities.yaml` 派生
- 首版只覆盖 Linux `bubblewrap` backend，不扩展到 Docker 或跨平台等价实现

## Goals / Non-Goals

**Goals:**

- 在现有 `bubblewrap` runtime 上增加一个可编译的 `SandboxKernelEnforcementPlan`
- 明确三层职责：`bubblewrap` 负责 namespace/mount，Landlock 负责文件对象边界，Seccomp 负责 syscall 面
- 让 proxy 在进入 JSON-RPC loop 前就完成 `no_new_privs` 与 proxy 级 Landlock 安装
- 让 `exec_command` 派生的命令子进程在真正 `exec` 用户命令前安装 child 级 Landlock/Seccomp
- 为 capability 配置增加 profile、兼容性要求和 fail-closed 行为
- 保持现有 tool routing 与 command session 语义尽量不变，只在权限不足时显式失败

**Non-Goals:**

- 本次不重做一套脱离 `bubblewrap` 的 namespace runtime
- 本次不实现域名/IP 级网络策略；Seccomp/Landlock 首版只覆盖 syscall 和文件对象边界
- 本次不让 Docker backend 同步获得 Landlock/Seccomp 支持
- 本次不把所有 browser/MCP/sidecar 子进程一次性改到新 child helper，首版优先覆盖 sandbox proxy 内的 command child

## Decisions

### 1. 继续以 `bubblewrap` 作为 namespace/mount 执行器，不把 Landlock/Seccomp 混进同一层抽象

新增 `SandboxKernelEnforcementPlan`，由现有 capability policy 编译出三部分：

- `BubblewrapNamespacePlan`
- `ProxyLandlockPlan`
- `CommandChildProfilePlan`

`bubblewrap` 继续负责 user/mount/pid/ipc/uts/net namespace 与 bind mount；Landlock 和 Seccomp 不再被视为“bubblewrap 参数的附属品”，而是 enforcement plan 的独立层。

Why:

- 当前项目里 namespace 这一层已经由 `bubblewrap` 承担，重复抽象没有价值
- Landlock/Seccomp 的安装时机和作用对象不同，合并成一套 path policy 会导致职责混乱
- 编译计划比运行时拼散装参数更可测试，也更方便记录 fail-closed 原因

Alternative considered:

- 继续扩展 `SandboxPathPolicy`，让它同时承担路径、syscall 和运行时兼容性判断。
  Rejected，因为它当前只是宿主侧请求规范化工具，不适合承载内核 enforcement 语义。

### 2. baseline Seccomp 由 `bubblewrap` 侧安装，proxy 与 child 再做更细收口

设计上分两级 Seccomp：

- baseline seccomp：在 `bubblewrap` 完成 namespace/mount setup 后安装到 proxy 所在进程，用来封死 mount/unshare/setns/ptrace/bpf 等逃逸 syscall
- child seccomp：在命令子进程真正 `exec` 用户命令前安装，比 proxy 更严格

基线 filter 必须允许 proxy 继续执行：

- `prctl(PR_SET_NO_NEW_PRIVS)`
- Landlock 相关 syscall
- 为 child 安装第二层 seccomp/landlock 所需 syscall
- 命令会话正常工作需要的 `fork/clone/execve/wait/pipe/epoll/futex`

Why:

- Seccomp 太早装在宿主侧会把 `bubblewrap` 自己的 setup 阶段锁死
- proxy 和 child 的 syscall 需求不同，一套 filter 很难同时做到安全和可用
- 先宽后窄的叠加方式更贴合现在“长生命周期 proxy + 多个命令 child”的结构

Alternative considered:

- 只给 proxy 装一层 seccomp，所有 command child 共用同一规则。
  Rejected，因为命令子进程通常可以更严格，长期运行 proxy 不应被最严格 profile 绑死。

### 3. proxy 启动时通过 plan 输入完成 `no_new_privs + Landlock`，而不是依赖环境变量或硬编码

`bubblewrap` 目前使用 `--clearenv` 启动 helper，因此 enforcement plan 不适合通过环境变量传递。设计上使用一份结构化 plan 输入给 proxy，proxy 在启动后先做：

1. 校验 plan 与本机 ABI/内核支持
2. `prctl(PR_SET_NO_NEW_PRIVS, 1, ...)`
3. 安装 proxy 级 Landlock
4. 再进入 JSON-RPC loop

如果 plan 要求的 Landlock ABI 或 seccomp 能力不满足，proxy 不进入服务状态，直接返回启动失败。

Why:

- `--clearenv` 已经让 env 传参不可靠
- plan 是结构化策略，不应被打散成多个零散 flag
- proxy 在读到第一条 JSON-RPC 请求前就应该已经被收口

Alternative considered:

- 把 enforcement 参数继续扩展到 `internal-sandbox proxy --flag ... --flag ...`
  Rejected，因为规则会很快变长，且 profile/plan 结构不适合展平为命令行参数。

### 4. 为 command child 新增独立 helper 入口，而不是只靠 `pre_exec`

新增隐藏 helper，例如 `openjarvis internal-sandbox exec`。proxy 在处理 `exec_command` 时，不再直接 `spawn /bin/sh ...`，而是统一调用这个 helper：

1. helper 读取目标 child profile
2. 设置 `no_new_privs`
3. 安装 child 级 Landlock
4. 安装 child 级 Seccomp
5. 最后 `execve` 真正 shell / command

Why:

- 现有命令执行同时覆盖 pipe 与 PTY 两条路径，统一 helper 比在多个 spawn 分支各自拼 `pre_exec` 更稳
- child helper 让 pipe/PTY 在 enforcement 上共享同一入口
- child 级 enforcement 明确把“proxy 自身能力”和“用户命令能力”分开

Alternative considered:

- 继续在 `spawn_pipe_command` / PTY 分支里用 `pre_exec` 各自安装 enforcement。
  Rejected，因为两条路径实现不同，后续 profile 演进容易分叉。

### 5. capability policy 明确区分 proxy profile、command profile 和兼容性要求

在 `config/capabilities.yaml` 中增加 bubblewrap 下的 enforcement 子段，至少表达：

- namespace 开关
- baseline seccomp profile
- proxy landlock profile
- command profile 映射
- `require_landlock`, `min_landlock_abi`, `require_seccomp`, `strict` 这类兼容性要求

worker 初始化时先编译 enforcement plan；如果 policy 显式要求而宿主不支持，则 fail fast。

Why:

- 现在的配置只表达 mount/path policy，无法区分 proxy 与 child 的不同权限面
- profile 化之后，未来可以继续给 browser/MCP/不同 thread kind 复用
- fail-closed 是这次方案的一部分，不应退化成“尽力而为但静默降级”

Alternative considered:

- 首版只做硬编码 profile，不在配置里暴露。
  Rejected，因为 capability policy 本来就是这条边界的配置中心，硬编码会让后续审计与切换更困难。

## Risks / Trade-offs

- [baseline seccomp 放得太严格会锁死 proxy 自己的后续安装能力] → baseline profile 明确保留 Landlock 与 child enforcement 所需 syscall，并为其增加失败回归测试。
- [proxy 与 child 两级 profile 可能出现配置不一致] → 统一由 `SandboxKernelEnforcementPlan` 编译，避免运行时分别读取和各自解释配置。
- [某些 Linux 环境关闭 userns 或不支持目标 Landlock ABI] → policy 增加显式兼容性字段；当 `strict=true` 时 fail fast，并在错误里指出缺失能力。
- [新增 child helper 会增加 command 执行链复杂度] → 保持现有 command session 结构不变，只把真正 `exec` 前的安全收口提到统一入口。
- [Landlock 首版只覆盖文件对象边界，无法替代完整网络策略] → 在 spec 和设计中明确这是文件对象 enforcement，不承诺域名/IP 级策略。

## Migration Plan

1. 扩展 `config/capabilities.yaml` 结构，加入 bubblewrap enforcement profile 和兼容性字段。
2. 在 `src/agent/sandbox.rs` 中新增 enforcement plan 编译层，并扩展 `bubblewrap` 初始化流程。
3. 扩展 `internal-sandbox proxy` 启动协议，使其接收 enforcement plan 并在 loop 前安装 proxy 级 Landlock。
4. 新增 `internal-sandbox exec` helper，把 command child 的 Landlock/Seccomp 安装收口到统一入口。
5. 让 `exec_command` 通过 child helper 派生命令，补齐 pipe/PTY 两条路径的 enforcement 回归。
6. 增加 Linux-only 集成测试，覆盖兼容性失败、policy 拒绝、proxy 启动失败和 child 被 profile 拒绝的显式报错。

Rollback strategy:

- 配置层把 backend 切回 `disabled` 或关闭 bubblewrap enforcement profile
- 若实现已上线但兼容性问题过多，可先保留 plan 编译与错误诊断，把 child helper 接入延后

## Open Questions

- baseline seccomp 是完全依赖 `bubblewrap` 的 seccomp FD 安装，还是允许一部分在 proxy 启动后补装；这取决于目标 `bwrap` 版本与部署环境兼容性。
- child helper 是否首版就要支持不同 thread kind / tool kind 的 profile 选择，还是先只做单一 `command-default`。
- browser/MCP 等会长期持有网络能力的子进程，后续是复用同一 profile 体系，还是单独建立 sidecar profile 家族。
