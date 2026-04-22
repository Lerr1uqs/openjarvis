---
title: Sandbox RPC Implementation Notes
status: proposal-reference
source_of_truth: openspec
notice: 本文仅作为提案参考，不作为当前实现事实依据；当前需求、设计与验收请以 openspec 为准。
---

# Sandbox RPC Implementation Notes

## 1. 目标

当前实现的目标是提供一个最小但可运行的“宿主机 -> proxy -> 沙箱内 bash”执行链路，并满足以下约束：

- 宿主机通过 JSON-RPC 请求 proxy，在沙箱内部执行 `bash`
- `bwrap` 只负责基础视图隔离，不负责 `/workspace` 的细粒度读写权限
- `/workspace` 的读写权限完全由 Landlock policy 决定
- `seccomp` 由 YAML profile 驱动，支持静态规则和 admin 动态追加的禁用 syscall
- 不同 agent 通过字符串 `agent_id` 区分，例如 `default`、`main`
- admin 可以动态修改某个 agent 的：
  - `writable_paths`
  - `disabled_syscalls`
- 动态 agent override 保存在沙箱内 `/runtime/agent-state/<agent_id>.json`
- bash 子进程不可读取 `/runtime/agent-state`

## 2. 当前组件

### 2.1 `start_proxy.sh`

职责：

- 编译 `sandbox_exec`
- 启动 `bwrap`
- 在沙箱内拉起 `proxy.py`

它负责的能力：

- user/pid/ipc/uts namespace
- 基础运行时只读挂载：
  - `/usr`
  - `/bin`
  - `/lib`
  - `/lib64`
- 将脚本目录只读挂到 `/opt/sandbox-rpc`
- 将宿主机 workspace 挂到 `/workspace`
- 将宿主机 runtime 挂到 `/runtime`
- 提供：
  - `--proc /proc`
  - `--dev /dev`
  - `--tmpfs /tmp`

边界：

- `bwrap` 不负责 `/workspace` 是只读还是可写
- `bwrap` 不做 agent 级别权限控制
- `bwrap` 不做动态策略更新

当前启动命令核心部分：

```bash
exec bwrap \
  --die-with-parent \
  --unshare-user \
  --unshare-pid \
  --unshare-ipc \
  --unshare-uts \
  --ro-bind /usr /usr \
  --ro-bind /bin /bin \
  --ro-bind /lib /lib \
  --ro-bind /lib64 /lib64 \
  --ro-bind "$SCRIPT_DIR" /opt/sandbox-rpc \
  --proc /proc \
  --dev /dev \
  --bind "$WORKSPACE_DIR" /workspace \
  --bind "$RUNTIME_DIR" /runtime \
  --ro-bind "$POLICY_PATH" /runtime/policy.yaml \
  --tmpfs /tmp \
  --chdir /workspace \
  /usr/bin/python3 /opt/sandbox-rpc/proxy.py \
    --socket-path /runtime/proxy.sock \
    --policy-path /runtime/policy.yaml \
    --runtime-state-dir /runtime/agent-state
```

### 2.2 `proxy.py`

职责：

- 在沙箱内监听 UNIX socket JSON-RPC
- 加载静态 policy
- 读取和写入 agent 的动态 state 文件
- 根据 `agent_id` 计算某次请求的最终权限
- 调起 `sandbox_exec`

当前支持的方法：

- `bash`
- `admin.get_agent`
- `admin.update_agent`

边界：

- proxy 是控制面入口
- proxy 自身不执行 seccomp / Landlock
- proxy 只负责为每次命令生成“将要施加”的权限配置
- 真正的限制是在 `sandbox_exec` 里对 bash 子进程安装

proxy 里最关键的三段逻辑是：

1. 读取文件态 agent override

```python
def load_agent_override(runtime_state_dir: str, agent_id: str) -> dict[str, Any] | None:
    path = agent_state_path(runtime_state_dir, agent_id)
    if not os.path.exists(path):
        return None
    with open(path, "r", encoding="utf-8") as f:
        raw = json.load(f)
    ...
```

2. 按 agent 构造最终策略

```python
def build_effective_policy(runtime_state: dict[str, Any], agent_id: str) -> dict[str, Any]:
    static_policy = runtime_state["static"]
    agent_cfg = get_effective_agent_state(runtime_state, agent_id)
    rules = list(static_policy["static_seccomp_rules"])
    if agent_cfg["disabled_syscalls"]:
        rules.append({
            "name": f"dynamic-disabled-syscalls-{agent_id}",
            "action": "deny",
            "errno": errno.EPERM,
            "reason": f"Dynamically disabled syscalls for agent {agent_id} via admin RPC.",
            "syscalls": list(agent_cfg["disabled_syscalls"]),
        })
    return {
        "seccomp": {"enabled": static_policy["seccomp_enabled"], "rules": rules},
        "landlock": {"enabled": static_policy["landlock_enabled"], "writable_paths": list(agent_cfg["writable_paths"])},
    }
```

3. admin RPC 更新文件态 override

```python
def handle_admin_update_agent(params: dict[str, Any], runtime_state: dict[str, Any]) -> dict[str, Any]:
    validate_admin_token(runtime_state, params.get("admin_token"))
    agent_id = require_agent_id(params.get("agent_id"))
    override = load_agent_override(runtime_state["runtime_state_dir"], agent_id) or {}
    ...
    save_agent_override(runtime_state["runtime_state_dir"], agent_id, override)
    ...
```

### 2.3 `sandbox_exec.c`

职责：

- 在 exec 前安装 Landlock
- 在 exec 前安装 seccomp
- 最后 `execvp(...)` 启动目标程序

它是“策略执行器”，不是“策略存储器”。

边界：

- 不关心 agent 是谁
- 不读取 YAML
- 不负责 admin 更新
- 只处理 proxy 传进来的最终规则

Landlock 的核心规则：

```c
if (add_path_rule(ruleset_fd, "/bin", kReadExecAccess) != 0 ||
    add_path_rule(ruleset_fd, "/usr", kReadExecAccess) != 0 ||
    add_path_rule(ruleset_fd, "/lib", kReadExecAccess) != 0 ||
    add_path_rule(ruleset_fd, "/lib64", kReadExecAccess) != 0 ||
    add_path_rule(ruleset_fd, "/workspace", kReadExecAccess) != 0 ||
    add_path_rule(ruleset_fd, "/tmp", kReadWriteAccess) != 0) {
    close(ruleset_fd);
    return -1;
}

for (size_t i = 0; i < config->writable_path_count; ++i) {
    if (add_path_rule(ruleset_fd, config->writable_paths[i], kReadWriteAccess) != 0) {
        close(ruleset_fd);
        return -1;
    }
}
```

seccomp 的核心安装逻辑：

```c
static int install_seccomp(const struct sandbox_config *config)
{
    scmp_filter_ctx ctx = seccomp_init(SCMP_ACT_ALLOW);
    ...
    for (size_t i = 0; i < config->seccomp_rule_count; ++i) {
        ...
        if (seccomp_rule_add_exact(ctx, config->seccomp_rules[i].errno_action,
                                   syscall_nr, 0) != 0) {
            ...
        }
    }
    if (seccomp_load(ctx) != 0) {
        ...
    }
    seccomp_release(ctx);
    return 0;
}
```

### 2.4 `remote_bash.py`

职责：

- 宿主机侧 agent 客户端
- 发送 `bash` RPC
- 在请求里带 `agent_id`

支持：

- 交互式 REPL
- 单次 `--command`
- `--agent-id`
- `:agent <id>` 在 REPL 中切换 agent

请求格式核心代码：

```python
request = {
    "jsonrpc": "2.0",
    "id": str(uuid.uuid4()),
    "method": "bash",
    "params": {
        "agent_id": agent_id,
        "command": command,
        "cwd": cwd,
        "timeout_ms": timeout_ms,
    },
}
```

### 2.5 `admin_client.py`

职责：

- 宿主机侧 admin 客户端
- 查询 agent 当前有效权限
- 更新 agent 的动态 override

支持：

- `get-agent`
- `update-agent`

调用 `update-agent` 时发送的数据结构：

```python
params = {
    "admin_token": args.admin_token,
    "agent_id": args.agent_id,
}
if args.writable_paths is not None:
    params["writable_paths"] = parse_csv(args.writable_paths)
if args.disabled_syscalls is not None:
    params["disabled_syscalls"] = parse_csv(args.disabled_syscalls)
response = send_request(sock, "admin.update_agent", params)
```

### 2.6 `policy.yaml`

职责：

- 提供静态启动配置
- 定义 seccomp 规则库
- 定义 seccomp profile
- 定义每个 agent 的初始状态
- 定义 admin token

边界：

- 只负责初始静态配置
- 不保存动态 override
- 动态 override 会写入 `/runtime/agent-state/*.json`

当前静态 policy 示例：

```yaml
admin:
  token: dev-admin

seccomp:
  enabled: true
  profile: deny-sysv-ipc

landlock:
  enabled: true
  writable_paths: []

agents:
  default:
    writable_paths: []
    disabled_syscalls: []
  main:
    writable_paths: []
    disabled_syscalls: []
```

## 3. 权限模型

## 3.1 `bwrap` 的职责

当前明确收敛为：

- `bwrap` 负责基础环境和视图
- `bwrap` 不负责 `/workspace` 的只读/可写切换

也就是说：

- `/workspace` 总是被 bind 进沙箱
- 但是否能读写，由后续 bash 子进程所安装的 Landlock 决定

这样做的好处：

- `bwrap` 配置稳定
- 动态 agent 权限不需要重启整个 proxy
- 权限切换变成“每次 bash 执行前装不同 Landlock 规则”

## 3.2 Landlock 的职责

Landlock 决定 bash 子进程能访问哪些文件路径。

当前策略：

- `/bin`、`/usr`、`/lib`、`/lib64`：只读 + 可执行
- `/workspace`：默认只读
- `/tmp`：可读可写
- `writable_paths`：额外提升为可读可写

注意：

- `/runtime` 没有被加入 Landlock 允许列表
- 所以 bash 子进程默认不能读取 `/runtime`
- 这就包括不能读取 `/runtime/agent-state`

这正是当前用来保护 agent 动态 state 文件的核心机制。

## 3.3 seccomp 的职责

seccomp 决定 bash 子进程允许调用哪些 syscall。

当前策略：

- 默认动作固定为 `allow`
- 规则只支持 `deny`
- 每条规则可以定义：
  - `errno`
  - `reason`
  - `syscalls`

静态 seccomp 规则来自 YAML：

- `rules.<rule_name>`
- `profiles.<profile_name>.applied_rules`

动态 seccomp 规则来自 admin：

- `disabled_syscalls`

proxy 会把动态禁用的 syscall 转换成一条额外的 seccomp deny 规则，然后和静态规则一起传给 `sandbox_exec`

## 4. 动态 agent state

## 4.1 存储位置

动态 state 文件保存在：

- `/runtime/agent-state/<agent_id>.json`

宿主机对应位置：

- `demo-user/runtime/agent-state/<agent_id>.json`

例如：

```json
{
  "writable_paths": ["/workspace"]
}
```

```json
{
  "disabled_syscalls": ["getpid"]
}
```

## 4.2 为什么放这里

原因是：

- proxy 进程运行在沙箱内，必须能频繁读写动态状态
- bash 子进程不应该读到这份控制面数据

`/runtime/agent-state` 满足这两个条件：

- proxy 可读可写
- bash 子进程默认因 Landlock 无法读取

## 4.3 文件内容

当前动态 state 允许保存：

- `writable_paths`
- `disabled_syscalls`

示例：

```json
{
  "writable_paths": [
    "/workspace"
  ]
}
```

或：

```json
{
  "disabled_syscalls": [
    "getpid"
  ]
}
```

## 4.4 生效方式

admin 更新后：

1. proxy 将 override 写入 `/runtime/agent-state/<agent_id>.json`
2. 后续新的 `bash` 请求到来时
3. proxy 读取该 agent 的 override
4. 和静态默认策略合并
5. 传给 `sandbox_exec`
6. `sandbox_exec` 对本次 bash 子进程装载新的 Landlock/seccomp

重要边界：

- 只影响后续新启动的命令
- 不影响已经在跑的子进程

## 5. RPC 设计

## 5.1 `bash`

方法名：

- `bash`

输入：

- `agent_id`
- `command`
- `cwd`
- `timeout_ms`
- `env`

作用：

- 为某个 agent 生成当前有效策略
- 在沙箱内执行 `/bin/bash -lc <command>`

## 5.2 `admin.get_agent`

方法名：

- `admin.get_agent`

输入：

- `admin_token`
- `agent_id`

作用：

- 返回某个 agent 当前有效的：
  - `writable_paths`
  - `disabled_syscalls`

## 5.3 `admin.update_agent`

方法名：

- `admin.update_agent`

输入：

- `admin_token`
- `agent_id`
- `writable_paths`
- `disabled_syscalls`

作用：

- 更新 agent 的文件态 override
- 立即影响后续命令

## 5.4 Admin RPC Schema

当前 admin RPC 走同一个 JSON-RPC 通道，但方法名带 `admin.` 前缀。

### `admin.get_agent`

请求示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "admin.get_agent",
  "params": {
    "admin_token": "dev-admin",
    "agent_id": "default"
  }
}
```

响应示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {
    "ok": true,
    "agent_id": "default",
    "writable_paths": [],
    "disabled_syscalls": []
  }
}
```

### `admin.update_agent`

请求示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "method": "admin.update_agent",
  "params": {
    "admin_token": "dev-admin",
    "agent_id": "default",
    "writable_paths": ["/workspace"],
    "disabled_syscalls": ["getpid"]
  }
}
```

响应示例：

```json
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "result": {
    "ok": true,
    "agent_id": "default",
    "writable_paths": ["/workspace"],
    "disabled_syscalls": ["getpid"]
  }
}
```

约束：

- `admin_token` 必须匹配 `policy.yaml` 中的 `admin.token`
- `agent_id` 必须是非空字符串
- `writable_paths` 如果提供，则会覆盖该 agent 现有文件态 override 中的同名字段
- `disabled_syscalls` 如果提供，则会覆盖该 agent 现有文件态 override 中的同名字段
- 未提供的字段保持原值不变

## 5.5 Agent State JSON Schema

动态 agent state 文件位于：

- `/runtime/agent-state/<agent_id>.json`

当前支持的 JSON 字段：

```json
{
  "writable_paths": ["/workspace"],
  "disabled_syscalls": ["getpid"]
}
```

字段语义：

- `writable_paths`
  - 类型：字符串数组
  - 语义：覆盖该 agent 当前额外可写路径集合
  - 约束：路径必须归一化后位于 `/workspace` 内
- `disabled_syscalls`
  - 类型：字符串数组
  - 语义：为该 agent 后续命令追加一条动态 seccomp deny 规则
  - 约束：每个元素必须是非空 syscall 名称

合并逻辑：

1. 先读取 `policy.yaml` 中 `agents.<agent_id>` 的默认值
2. 再读取 `/runtime/agent-state/<agent_id>.json`
3. 文件态字段如果存在，则覆盖默认值
4. 生成本次请求的最终有效策略

这意味着：

- 文件态 override 优先级高于静态默认值
- 删除 state 文件即可回退到静态默认值

## 5.6 错误码与返回语义

当前实现没有自定义完整错误码枚举，而是使用 JSON-RPC error object 的 `code + message`。

当前主要返回形态如下：

### RPC 成功

格式：

```json
{
  "jsonrpc": "2.0",
  "id": "req-id",
  "result": {
    "ok": true
  }
}
```

### 参数错误

典型场景：

- `agent_id` 为空
- `writable_paths` 不是数组
- `disabled_syscalls` 元素不是字符串
- `cwd` 越界

格式：

```json
{
  "jsonrpc": "2.0",
  "id": "req-id",
  "error": {
    "code": -32602,
    "message": "..."
  }
}
```

### 方法不存在

格式：

```json
{
  "jsonrpc": "2.0",
  "id": "req-id",
  "error": {
    "code": -32601,
    "message": "unsupported method: ..."
  }
}
```

### JSON 解析错误

格式：

```json
{
  "jsonrpc": "2.0",
  "id": null,
  "error": {
    "code": -32700,
    "message": "parse error: ..."
  }
}
```

### bash 执行结果

`bash` 方法即使命令退出码非零，通常仍返回 `result` 而不是 JSON-RPC `error`：

```json
{
  "jsonrpc": "2.0",
  "id": "req-id",
  "result": {
    "ok": false,
    "agent_id": "default",
    "exit_code": 1,
    "stdout": "",
    "stderr": "/bin/bash: ... Permission denied\n",
    "timed_out": false
  }
}
```

含义：

- JSON-RPC `error`
  - 代表请求本身无效，或 proxy 级别处理失败
- `result.ok == false`
  - 代表请求已正确执行到 bash 层，但命令失败、权限被拒或超时

这种区分的好处是：

- admin / agent 客户端可以明确区分“协议错误”和“命令执行失败”
- 不需要把所有 bash 非零退出都提升为 RPC 层异常

## 6. 当前已实现的 feature

### 6.1 基础沙箱执行

已实现：

- 宿主机发 RPC
- 沙箱内执行 `bash`
- stdout/stderr 返回
- timeout 返回

### 6.2 按 agent_id 区分权限

已实现：

- agent id 是普通字符串
- 例如 `default`、`main`
- 同一个 proxy 可服务多个 agent

### 6.3 `/workspace` 默认只读

已实现：

- `policy.yaml` 中 `landlock.writable_paths: []`
- 所以 agent 默认对 `/workspace` 只读

### 6.4 动态把 `/workspace` 改为可写

已实现：

- admin 通过：
  - `update-agent --agent-id default --writable-paths /workspace`
- 后续 `default` agent 就可写 `/workspace`

### 6.5 不同 agent 权限互不影响

已实现：

- 给 `default` 加 `/workspace` 可写
- `main` 仍保持只读

### 6.6 动态禁用 syscall

已实现：

- admin 通过：
  - `update-agent --agent-id main --disabled-syscalls getpid`
- 后续 `main` agent 的 `getpid` 返回 `EPERM`

### 6.7 `reason` 必填

已实现：

- 每条静态 seccomp 规则必须有清晰的 `reason`
- 否则 proxy 启动失败

### 6.8 `errno` 支持名字

已实现：

- 可以写 `EPERM`
- 不必写数字 `1`

## 7. 当前验证结果

已经实际验证过：

### 7.1 `/workspace` 默认只读

命令：

```bash
python3 sandbox-rpc/remote_bash.py \
  --socket-path demo-user/runtime/proxy.sock \
  --agent-id default \
  --command 'echo blocked > /workspace/state-test.txt'
```

结果：

- 返回 `Permission denied`

### 7.2 admin 动态改为可写后生效

命令：

```bash
python3 sandbox-rpc/admin_client.py \
  --socket-path demo-user/runtime/proxy.sock \
  --admin-token dev-admin \
  update-agent \
  --agent-id default \
  --writable-paths /workspace
```

后续命令：

```bash
python3 sandbox-rpc/remote_bash.py \
  --socket-path demo-user/runtime/proxy.sock \
  --agent-id default \
  --command 'echo allowed > /workspace/state-test.txt && cat /workspace/state-test.txt'
```

结果：

- 输出 `allowed`
- 宿主机 workspace 中出现对应文件

### 7.3 bash 子进程不可读 `/runtime/agent-state`

命令：

```bash
python3 sandbox-rpc/remote_bash.py \
  --socket-path demo-user/runtime/proxy.sock \
  --agent-id default \
  --command 'cat /runtime/agent-state/default.json'
```

结果：

- `Permission denied`

### 7.4 动态禁用 syscall 生效

命令：

```bash
python3 sandbox-rpc/admin_client.py \
  --socket-path demo-user/runtime/proxy.sock \
  --admin-token dev-admin \
  update-agent \
  --agent-id main \
  --disabled-syscalls getpid
```

后续验证：

- `getpid` 返回 `EPERM`

## 8. 当前设计边界

### 8.1 已明确支持

- 多 agent 共用一个 proxy
- agent id 为普通字符串
- `/workspace` 权限完全由 Landlock 控制
- 动态读写权限更新
- 动态 syscall 禁用
- 文件态持久化动态 override

### 8.2 尚未实现

- supervisor
- cgroups
- 网络 namespace 的精细控制
- admin token 的更强认证机制
- agent token / agent 身份校验体系
- `read/write/edit` RPC
- override 删除/重置接口
- 动态 state 的并发写冲突处理
- 软链接更严格的 `openat2` 路径解析

## 9. 组件边界总结

### `bwrap`

负责：

- 基础 namespace
- 基础挂载
- `/proc` `/dev` `/tmp`

不负责：

- `/workspace` 的读写权限
- agent 权限切换
- 动态策略更新

### `proxy`

负责：

- 控制面
- policy 加载
- 动态 agent state 文件读写
- admin RPC
- 每请求生成最终权限配置

不负责：

- 直接强制 syscall/path 限制

### `sandbox_exec`

负责：

- 执行面
- 在 exec 前把 proxy 决定好的策略真正装进内核

不负责：

- policy 存储
- admin 更新
- agent 状态管理

### `Landlock`

负责：

- `/workspace` 读写权限
- 屏蔽 bash 子进程读取 `/runtime/agent-state`

### `seccomp`

负责：

- deny 指定 syscall
- 静态规则 + 动态 agent 禁用 syscall

## 10. 推荐后续工作

建议下一步优先做：

1. `admin.reset_agent`

作用：

- 删除 `/runtime/agent-state/<agent_id>.json`
- 恢复为 `policy.yaml` 中的默认 agent 配置

2. 更严格的路径解析

作用：

- 用 `openat2` 风格约束路径
- 进一步收紧软链接/path traversal 风险

3. 为动态 state 文件增加锁或原子更新说明

作用：

- 避免多 admin 并发修改时互相覆盖
