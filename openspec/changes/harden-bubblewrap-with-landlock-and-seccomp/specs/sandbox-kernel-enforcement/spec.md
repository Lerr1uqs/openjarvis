## ADDED Requirements

### Requirement: Bubblewrap sandbox SHALL enforce kernel restrictions in layered stages
系统 SHALL 在现有 `bubblewrap` sandbox 中把 namespace/mount、Landlock 和 Seccomp 视为三层独立 enforcement，而 SHALL NOT 继续只依赖 mount 视图与宿主侧路径校验表达全部 capability policy。

#### Scenario: Worker compiles a layered enforcement plan
- **WHEN** worker 根据 `config/capabilities.yaml` 初始化 `bubblewrap` backend
- **THEN** 系统 SHALL 先编译一份结构化 enforcement plan
- **THEN** 该 plan SHALL 至少区分 namespace/mount、proxy 级 Landlock 和 command child 级 enforcement

### Requirement: Sandbox proxy SHALL install proxy-level kernel restrictions before serving requests
系统 SHALL 在 `internal-sandbox proxy` 进入 JSON-RPC loop 前完成 `no_new_privs` 与 proxy 级 Landlock 安装，而 SHALL NOT 在开始处理请求后才补装这些限制。

#### Scenario: Proxy starts with a valid enforcement plan
- **WHEN** `bubblewrap` backend 使用有效 enforcement plan 启动 `internal-sandbox proxy`
- **THEN** proxy SHALL 在处理第一条 JSON-RPC 请求前先设置 `no_new_privs`
- **THEN** proxy SHALL 安装 proxy 级 Landlock，然后才进入服务状态

#### Scenario: Proxy enforcement requirements are unavailable
- **WHEN** proxy plan 显式要求的 Landlock 或 seccomp 内核能力在当前环境不可用
- **THEN** proxy SHALL 启动失败并返回明确错误
- **THEN** 系统 SHALL NOT 以未收口状态继续提供 JSON-RPC 服务

### Requirement: Command child SHALL apply child-specific restrictions before executing user commands
系统 SHALL 在 `exec_command` 派生命令子进程时，先对 child 安装 profile 对应的 Landlock 与 Seccomp，再执行真正的 shell/command，而 SHALL NOT 让用户命令直接继承 proxy 的宽权限窗口。

#### Scenario: Command session launches under a declared child profile
- **WHEN** sandbox 内的 `exec_command` 以一个已声明的 command profile 启动命令
- **THEN** 系统 SHALL 先进入 child enforcement 入口并安装该 profile 的 Landlock 与 Seccomp
- **THEN** 只有 enforcement 安装成功后，系统 SHALL 执行真正的用户命令

#### Scenario: Command child violates its profile
- **WHEN** 用户命令访问 child profile 未授权的文件对象或触发被禁止的 syscall
- **THEN** 命令执行 SHALL 显式失败
- **THEN** 系统 SHALL NOT 回退到宿主机执行，也 SHALL NOT 放宽为 proxy 级权限继续运行

### Requirement: Baseline Seccomp SHALL block sandbox escape syscalls while preserving child enforcement setup
系统 SHALL 在 proxy 所在的 sandbox 进程上安装 baseline seccomp，用于拒绝 mount、namespace 操作和其他逃逸相关 syscall；同时该 baseline filter SHALL 保留 child enforcement 所需的最小 syscall 面。

#### Scenario: Baseline filter denies escape-oriented syscalls
- **WHEN** proxy 或其后代尝试触发 baseline profile 明确禁止的逃逸类 syscall
- **THEN** 系统 SHALL 依照 seccomp profile 的拒绝动作阻止该调用

#### Scenario: Baseline filter still allows child enforcement installation
- **WHEN** proxy 为 command child 安装第二层 Landlock 与 Seccomp
- **THEN** baseline seccomp SHALL 允许完成该安装流程所需的最小 syscall
- **THEN** child enforcement SHALL NOT 因 baseline filter 过早收紧而无法建立
