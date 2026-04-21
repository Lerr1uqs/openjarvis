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

## 当前未实现或待补齐的问题

### 1. baseline seccomp denylist 未覆盖完整逃逸面

这是当前最重要的问题。

现有 baseline denylist 只覆盖了传统 syscall，例如：

- `mount`
- `umount2`
- `pivot_root`
- `setns`
- `unshare`
- `bpf`
- `ptrace`

但还没有覆盖新 mount API 和部分 namespace 相关入口，例如：

- `open_tree`
- `move_mount`
- `fsopen`
- `fsmount`
- `fspick`
- `mount_setattr`
- `clone` / `clone3` 上与 `CLONE_NEW*` 相关的参数级限制

这意味着当前 spec 对“baseline seccomp 会阻止 sandbox escape syscall”的承诺强于实际实现，需要继续补齐。

### 2. seccomp denylist 目前不支持自定义

当前配置只能引用 builtin seccomp profile 名称，不能在 `capabilities.yaml` 中直接声明自定义 syscall denylist，也不能扩展参数级规则。

这意味着：

- 不能通过改配置补齐缺失 syscall。
- 如果要补齐 denylist，必须修改代码里的 builtin profile 实现。

### 3. baseline seccomp 缺少直接回归测试

当前已有测试主要覆盖：

- 配置解析
- proxy 启动失败/成功路径
- readonly profile 对 workspace 写入的拒绝
- pipe/PTY 两条命令链路都经过 child helper

但还缺少直接验证“某个 escape-oriented syscall 会被 baseline seccomp 拒绝”的测试，因此 seccomp 覆盖不完整时，CI 目前不一定能第一时间发现。

### 4. 能力覆盖范围仍是首版

当前 sandbox kernel enforcement 首版主要覆盖 sandbox 内的 command child。

以下范围暂未继续扩展：

- Docker backend 的等价 Landlock/Seccomp 支持
- browser sidecar / MCP sidecar 的同类 child profile 体系
- 非 Linux 平台的等价实现

## 建议明天优先处理的事项

1. 先补齐 builtin baseline seccomp denylist，至少覆盖新 mount API 和 namespace 逃逸相关 syscall。
2. 增加 baseline seccomp 的集成测试，直接验证拒绝行为，而不是只验证 profile 可编译。
3. 再决定是否要设计“可配置 seccomp profile / denylist”能力，避免以后每次补 syscall 都需要改代码。
