## Context

当前 `BubblewrapSandbox` 已经具备长期存活的 JSON-RPC proxy，以及 `read_workspace_text` / `write_workspace_text` 两个文件原语；但 tool handler 仍然直接操作宿主机：

- `read` / `write` / `edit` 直接调用 `std::fs`
- `exec_command` / `write_stdin` / `list_unread_command_tasks` 直接调用宿主机 `CommandSessionManager`
- `AgentWorker` 虽然持有 `Sandbox`，但 `ToolRegistry` 和 `ToolCallContext` 并不能把这个运行时传给具体工具

因此当前的 sandbox 只是真实存在于 worker 层，尚未成为 core tool 的执行边界。

## Goals / Non-Goals

**Goals:**

- 让 `ToolCallContext` 能拿到当前 worker 安装的 `Sandbox`
- 让 `read`、`write`、`edit` 在 sandbox 开启时通过 JSON-RPC proxy 完成文件访问
- 让 `exec_command` 及其续写相关能力在 sandbox 开启时通过 JSON-RPC proxy 完成命令会话
- 保持 sandbox 关闭时的现有宿主机语义不变
- 保持 thread-scoped command session 的隔离语义和现有返回结构稳定

**Non-Goals:**

- 本次不把 `bash`、memory、browser、MCP sidecar 一起迁入 sandbox proxy
- 本次不修改 capability 配置模型，只复用现有 `sandbox.backend` 选择
- 本次不引入新的容器后端，也不扩展到 Docker 真实执行

## Decisions

### 1. `ToolRegistry` 持有当前 active sandbox，并在 `ToolCallContext` 中注入

`AgentWorker` 构造出真实 `Sandbox` 后，会把该实例安装到 `ToolRegistry`。线程侧的 `call_tool_with_registry(...)` 在构造 `ToolCallContext` 时一并带上 sandbox，这样 tool handler 无需感知 worker 本身。

这样可以避免把 sandbox 再塞进 `Thread` 持久态，也不需要让每个 tool handler 自己全局查找 worker。

Alternative considered:

- 把 sandbox 放进 `Thread` 的 request runtime 状态。
  Rejected，因为 sandbox 是 worker/runtime 级依赖，不属于线程快照语义，放进 `ToolRegistry` 更符合当前职责边界。

### 2. 文件工具直接复用现有 sandbox 文件 RPC，`edit` 通过“读 + 替换 + 写”组合实现

`read`、`write` 直接调用 `Sandbox::read_workspace_text` / `Sandbox::write_workspace_text`。`edit` 不新增专门的 replace RPC，而是在 host 侧保留现有文本匹配逻辑，只把读取和写回动作都走 sandbox RPC。

这样可以保持 `edit` 当前的匹配计数、错误文案和首个匹配替换语义不变，同时避免在 proxy 中复制一套文本替换规则。

Alternative considered:

- 新增 `fs.replace_text` RPC，把完整 edit 语义放进 proxy。
  Rejected，因为这会把 `edit` 的业务规则复制到 proxy 侧，扩大协议面，而本次只需要保证副作用通过 sandbox 完成。

### 3. 命令会话直接在 proxy 内复用现有 `CommandSessionManager`

proxy 进程本身是长期存活的，因此最合适的做法是在 proxy 内创建一个 `CommandSessionManager`，并新增 JSON-RPC 方法承接：

- `command.exec`
- `command.write_stdin`
- `command.list_unread_tasks`

这样可复用当前的 session 生命周期、TTY 支持、未读输出聚合和线程隔离逻辑，而不是在 sandbox 层重写一套轻量版命令管理器。

Alternative considered:

- 把 `exec_command` 简化成一次性同步命令 RPC，不支持 `write_stdin` / `list_unread_command_tasks`。
  Rejected，因为这会破坏现有 tool 契约，尤其是后台 session 与交互式命令。

### 4. `exec_command.workdir` 在 host 侧先过宿主路径策略，再以 workspace 相对路径进入 proxy

当 sandbox 开启时，`exec_command` 的 `workdir` 不能直接把宿主机路径原样传入沙箱。host 侧会先用现有 `SandboxPathPolicy` 校验路径，再转换成相对 `workspace_root` 的路径字符串发给 proxy，proxy 再解析成沙箱内 `/workspace/...` 路径。

这样可以复用现有“敏感目录限制 + 上级目录逃逸限制”，并避免把宿主绝对路径泄露成沙箱内部契约。

Alternative considered:

- 把宿主绝对路径直接传给 proxy。
  Rejected，因为 proxy 运行在沙箱内部，宿主绝对路径在语义上不稳定，而且会削弱现有路径策略边界。

### 5. `bash` 暂不迁移，本次只兑现用户明确要求的 core tool 集

虽然 `bash` 也是本地执行路径，但当前用户本轮明确要求的是 `exec_command`、`read`、`edit`、`write`。因此本次实现把规格和改动面控制在这四个工具及其 command session 续写工具上，不顺带扩大到 `bash`。

Alternative considered:

- 一起把 `bash` 迁到 proxy。
  Rejected，因为这会扩大实现面和测试面，而不是完成当前这轮明确需求。

## Risks / Trade-offs

- [proxy 内命令会话状态变多] -> 复用现有 `CommandSessionManager`，避免另起一套不成熟的 session 状态机
- [sandbox 模式下路径限制会让历史上可用的绝对路径失败] -> 在 spec 中明确这是 sandbox 生效后的预期边界，不做静默回退
- [`edit` 在 host 侧保留文本匹配逻辑，会产生一次额外 RPC 往返] -> 优先保持行为一致性；当前文本工具体量很小，这个代价可接受
- [`bash` 仍未进入 sandbox] -> 在 spec 和实现范围里明确排除，避免产生“全量工具已沙箱化”的误解

## Migration Plan

1. 给 `ToolRegistry` / `ToolCallContext` 增加 sandbox 注入能力，并由 `AgentWorker` 安装当前 sandbox
2. 扩展 sandbox JSON-RPC 协议，支持 command session 三个方法
3. 让 proxy 内复用 `CommandSessionManager` 承接命令执行与续写
4. 迁移 `read`、`write`、`edit`、`exec_command`、`write_stdin`、`list_unread_command_tasks`
5. 补齐 sandbox on/off、路径限制、真实 Bubblewrap command session 的回归测试

Rollback strategy:

- 如果需要回滚，只需移除 tool context 中的 sandbox 注入以及工具分支逻辑，sandbox runtime 本身可保留
- 配置切回 `disabled` 时继续使用现有宿主机路径，无需数据迁移

## Open Questions

- 是否要在下一轮把 `bash` 明确迁到相同 command RPC 上，避免 `exec_command` 与 `bash` 语义分裂
- `exec_command` 在 sandbox 模式下是否需要额外暴露“实际沙箱 workdir”到 metadata，当前设计先保持原有字段不变
