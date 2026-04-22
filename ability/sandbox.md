# Sandbox 能力文档

本文档描述当前项目中 sandbox 相关能力的实现边界、配置入口和已知待补齐问题。

## 概述

当前 sandbox 能力以 Linux 下的 `bubblewrap` backend 为主，目标是在现有工具路由和命令执行链路上增加更明确的运行时隔离与内核级收口。

当前实现采用三层模型：

1. `bubblewrap` 负责 namespace 和 mount 视图。
2. `internal-sandbox proxy` 负责 JSON-RPC 转发、工作区路径边界和 proxy 级 enforcement。
3. `internal-sandbox exec` helper 负责在真正执行用户命令前安装 child 级 enforcement。

## 当前提供的能力

### 1. 工作区同步与路径边界

- sandbox 内部把宿主工作区挂载到 `/workspace`。
- `read`、`write`、`edit` 等文件类工具通过 proxy 读写当前工作区。
- 显式 `/tmp` 路径允许使用，默认仍以工作区为主。
- 支持 `restricted_host_paths` 和 `allow_parent_access` 控制宿主路径逃逸边界。

### 2. 命令执行统一经由 sandbox proxy

- `exec_command`
- `write_stdin`
- `list_unread_command_tasks`

以上命令类工具在 sandbox 开启后，都通过 proxy 内部的 command session runtime 执行，而不是直接在宿主侧启动子进程。

### 3. 分层 kernel enforcement

当前已经接入以下收口能力：

- namespace/mount: 由 `bubblewrap` 负责 user/ipc/pid/uts/net namespace 开关和挂载视图。
- proxy 级收口: `internal-sandbox proxy` 在进入 JSON-RPC loop 前安装 `no_new_privs`、proxy 级 Landlock 和 baseline seccomp。
- command child 级收口: `internal-sandbox exec` helper 在真正 `exec` 用户命令前安装 `no_new_privs`、child 级 Landlock 和 child 级 seccomp。

### 4. command profile

当前内置了两类 command profile：

| profile | Landlock | Seccomp | 说明 |
|---------|----------|---------|------|
| `default` | `command-default` | `command-default-v1` | 默认命令权限，允许工作区读写 |
| `readonly` | `command-readonly` | `command-readonly-v1` | 只读工作区，命令不能写回 workspace |

当前配置里的 `selected_profile` 会作为 sandbox 默认 command profile 生效。

### 5. fail-closed 兼容性配置

当前支持以下兼容性配置：

- `require_landlock`
- `min_landlock_abi`
- `require_seccomp`
- `strict`

当策略显式要求某项能力且当前内核或运行环境不支持时，sandbox 会显式失败，而不是静默降级继续运行。

### 6. 调试与可观测性

关键动作已经带有日志，便于定位 sandbox 初始化和 enforcement 安装问题，主要包括：

- kernel enforcement plan 编译
- proxy enforcement 安装
- command child enforcement 安装
- command session 启动与输出收集

## 配置入口

当前配置入口位于 `config/capabilities.yaml` 的 `sandbox.bubblewrap` 段，主要字段包括：

| 字段 | 说明 |
|------|------|
| `namespaces` | namespace 开关 |
| `baseline_seccomp_profile` | proxy 基线 seccomp profile |
| `proxy_landlock_profile` | proxy 级 Landlock profile |
| `command_profiles.selected_profile` | 默认 command profile |
| `command_profiles.profiles` | 逻辑 profile 到 builtin profile 的映射 |
| `compatibility` | 内核能力与 fail-closed 要求 |

## 近期已补齐的事项

### 1. seccomp 已补齐新 mount API，并补强 namespace 逃逸相关规则

当前 seccomp profile 已补齐以下覆盖：

- 传统 mount / namespace 逃逸 syscall，如 `mount`、`umount2`、`pivot_root`、`setns`、`unshare`
- 新 mount API，如 `open_tree`、`move_mount`、`fsopen`、`fsmount`、`fspick`、`mount_setattr`
- `clone` 上与 `CLONE_NEW*` 相关的 namespace flag
- command child profile 下的 `clone3`

其中：

- proxy baseline profile 负责拦截传统逃逸 syscall、新 mount API 和 `clone` 的 `CLONE_NEW*` 参数位
- command child profile 在此基础上额外 deny `clone3`
- `clone3` 因 namespace flag 位于用户态结构体中，seccomp 无法安全按位检查，因此 child profile 里按整条 syscall deny 处理

### 2. baseline seccomp 已增加直接回归测试

当前已新增 syscall probe 回归测试，直接验证 sandbox 命令链路会对 escape-oriented syscall 返回 `EPERM`，而不只是验证 profile 可以被编译。

当前测试会优先覆盖宿主环境上可观测的 probe case，包括：

- 新 mount API
- `clone3`

## 当前仍未实现或待补齐的问题

### 1. proxy baseline 仍未直接 deny `clone3`

当前 proxy baseline 还不能直接 deny `clone3`。

原因是：

- proxy 在处理 `command.exec` 时仍需要依赖当前运行时/标准库的进程拉起路径
- 当前这条路径在宿主环境上会用到 `clone3`
- 如果直接把 `clone3` 放进 proxy baseline denylist，会导致 command child 无法被正常拉起

当前做法是：

- proxy baseline 先覆盖传统逃逸 syscall、新 mount API 和 `clone` 的 namespace flag
- `clone3` 由 command child profile 在 helper 进程真正 `exec` 用户命令前收口

### 2. seccomp denylist 目前不支持自定义

当前配置只能引用 builtin seccomp profile 名称，不能在 `capabilities.yaml` 中直接声明自定义 syscall denylist，也不能扩展参数级规则。

这意味着：

- 不能通过改配置补齐缺失 syscall。
- 如果要补齐 denylist，必须修改代码里的 builtin profile 实现。

### 3. 能力覆盖范围仍是首版

当前 sandbox kernel enforcement 首版主要覆盖 sandbox 内的 command child。

以下范围暂未继续扩展：

- Docker backend 的等价 Landlock/Seccomp 支持
- browser sidecar / MCP sidecar 的同类 child profile 体系
- 非 Linux 平台的等价实现

## 后续事项

1. 再决定是否要设计“可配置 seccomp profile / denylist”能力，避免以后每次补 syscall 都需要改代码。
2. 评估是否要把 profile 体系扩展到 browser sidecar、MCP sidecar 等长期子进程。
