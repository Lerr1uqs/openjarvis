## Why

当前 sandbox runtime 已经可以通过 Bubblewrap + JSON-RPC proxy 提供基础文件原语，但真正对 Agent 暴露的 `read`、`write`、`edit`、`exec_command` 仍然直接作用于宿主机。这会让“开启 sandbox”只停留在底层能力存在，而不是实际工具执行边界生效。

这次变更要把 core tool 的真实副作用切到 sandbox proxy 上：一旦启用 sandbox，这些工具的文件访问与命令执行都必须通过 JSON-RPC 进入沙箱并返回结果，而不是继续在宿主机直接执行。

## What Changes

- 新增一份聚焦 core tool sandbox 路由的规格，明确 `read`、`write`、`edit`、`exec_command` 在 sandbox 开启后的执行语义。
- 扩展现有 sandbox JSON-RPC 协议，补齐文本替换与命令执行/轮询所需原语。
- 将 builtin `read`、`write`、`edit`、`exec_command` 从直接访问宿主机改为优先通过 `Sandbox` 统一入口执行。
- 保持 sandbox 关闭时的现有宿主机行为不变；保持 `write_stdin` / `list_unread_command_tasks` 与 command session 生命周期语义兼容。
- 为真实 Bubblewrap 与 sandbox 关闭两种模式补充回归测试，确保不会静默回退到宿主机执行。

## Capabilities

### New Capabilities
- `sandbox-tool-routing`: 定义 core tool 在 sandbox 开启后必须通过 JSON-RPC proxy 进入沙箱执行的行为契约。

### Modified Capabilities

## Impact

- Affected code: `src/agent/sandbox.rs`、`src/agent/tool/read.rs`、`src/agent/tool/write.rs`、`src/agent/tool/edit.rs`、`src/agent/tool/command/**`、`src/agent/tool/mod.rs`、`src/agent/worker.rs`
- Affected tests: `tests/agent/sandbox.rs`、`tests/agent/tool/**`、`tests/agent/worker.rs`
- Runtime impact: 当 capability 选择 `bubblewrap` 时，core tool 不再直接读写宿主文件或直接在宿主机启动命令
- Compatibility impact: sandbox 关闭时继续保持当前宿主机执行语义；sandbox 开启但 backend 不可用时显式失败
