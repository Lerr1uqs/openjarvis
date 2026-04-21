## Why

当前 `bubblewrap` sandbox 已经提供 namespace、mount 和 JSON-RPC proxy 这层隔离，但真正的 capability policy 仍主要停留在路径映射和宿主侧校验，缺少基于内核的对象级与 syscall 级收口。随着 `exec_command`、core tool 路由和后续更多 sidecar/runtime 能力进入 sandbox，这个缺口会让“看得到的隔离边界”和“实际可越界的内核能力”不一致。

这次变更要把现有 `bubblewrap` 从“namespace/mount 视图隔离”升级为“namespace + Landlock + Seccomp 的分层 enforcement runtime”，让 proxy 与 command child 在既有架构内获得更明确、更可审计的最小权限约束。

## What Changes

- 在现有 `bubblewrap` backend 之上新增一层可编译的 kernel enforcement plan，把 capability policy 编译为 `bwrap namespace/mount`、`Landlock` 和 `Seccomp` 三类执行计划。
- 为 `config/capabilities.yaml` 增加 `bubblewrap` 下的内核 enforcement 配置，包括 namespace 开关、baseline seccomp profile、proxy/command child 的 Landlock profile 与 command capability profile 选择。
- 扩展 `internal-sandbox proxy` 启动协议，使 proxy 能在进入 JSON-RPC loop 之前读取 enforcement plan、设置 `no_new_privs` 并安装 proxy 级 Landlock。
- 为 sandbox 内的命令子进程引入独立的 child enforcement 入口，使 `exec_command` 能在 proxy 内继续派生命令时，额外安装 child 级 Landlock 与 Seccomp，而不是只共享 proxy 进程的宽权限。
- 明确 bubblewrap、Landlock、Seccomp 之间的分层职责：`bubblewrap` 负责 namespace/mount 视图，Landlock 负责文件对象边界，Seccomp 负责 syscall 面。
- **BREAKING**: 当 capability policy 显式要求 Landlock ABI、seccomp filter 或 namespace 功能而当前内核/宿主环境不满足时，`bubblewrap` backend 将 fail fast，而不是继续以较弱隔离运行。

## Capabilities

### New Capabilities
- `sandbox-kernel-enforcement`: 定义基于现有 `bubblewrap` runtime 的 namespace、Landlock 与 Seccomp 分层收口语义，以及 proxy/command child 两级 enforcement 行为。

### Modified Capabilities
- `sandbox-capability-policy`: capability policy 增加 bubblewrap 下的内核 enforcement 配置、profile 选择和 fail-closed 语义。
- `sandbox-runtime`: worker 初始化 bubblewrap 时需要编译并安装 baseline enforcement plan，而不是只拼接 mount/path policy。
- `sandbox-jsonrpc-proxy`: proxy 启动契约增加 enforcement plan 传递、`no_new_privs` 和 proxy 级 Landlock 安装。

## Impact

- Affected code: `src/agent/sandbox.rs`、`src/cli.rs`、`src/cli_command/internal.rs`、`src/agent/tool/command/process.rs`、`src/agent/tool/command/session.rs`
- Affected tests: `tests/agent/sandbox.rs`、新增面向 Landlock/Seccomp/profile 失败路径的 sandbox 集成测试
- Affected dependencies: 需要引入 Linux Landlock/Seccomp 相关 userspace crate 或最小 syscall 封装；运行环境需要支持相应内核能力
- Runtime impact: `bubblewrap` backend 将从单层 proxy runtime 升级为“baseline proxy + child-specific enforcement”双层执行模型
- Operational impact: `config/capabilities.yaml` 需要能表达更细的 bubblewrap enforcement policy，且 Linux 环境不满足要求时启动会显式失败
