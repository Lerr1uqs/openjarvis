## Why

当前 `sandbox` 仍然只是 `DummySandboxContainer` 占位对象，所有文件访问和工具自有子进程依然直接落在宿主机上，既没有统一的执行边界，也无法为后续多后端隔离方案建立稳定抽象。现在 browser 子线程、shell、memory、未来的 tool-owned sidecar 都已经形成了清晰的副作用入口，正是把“宿主执行”升级为“可配置沙箱执行”的合适时机。

这次变更不仅要把 Bubblewrap 真正接进来，还要把“agent 如何和沙箱对话”“全局用户如何声明沙箱 capability”“宿主敏感目录和上级目录如何受限”一次性明确下来，避免后面又回到在单个工具里零散补安全边界的老路。

## What Changes

- 新增统一 `Sandbox` trait，并把 `AgentWorker` 当前持有的占位沙箱替换为可扩展的运行时后端选择。
- 实现 Bubblewrap 后端，使用长期存活的 JSON-RPC proxy 在 agent 与 bwrap 内 helper 之间桥接文件和进程操作。
- 保留 Docker 作为并列后端枚举与 trait 实现入口，但当前阶段明确保持 `unimplemented!()`。
- 新增全局 capability 配置文件 `config/capabilities.yaml`，为全体用户声明默认沙箱能力、默认同步目录、敏感目录限制和上级目录访问限制。
- 将默认同步工作区固定为当前工作区根 `.`，并允许显式 `/tmp/...` 绝对路径；通过 JSON-RPC 在这些允许路径内修改文件后，宿主机可以直接观察到同步结果。
- 约束 Bubblewrap 沙箱对宿主敏感目录和工作区上级目录的访问，拒绝越权映射或静默穿透。

## Capabilities

### New Capabilities
- `sandbox-runtime`: 定义可扩展的沙箱 trait、Bubblewrap 后端和保留的 Docker 后端入口，以及 worker/runtime 如何持有和初始化沙箱。
- `sandbox-capability-policy`: 定义面向全体用户的 `config/capabilities.yaml` 能力配置、默认同步目录 `.`、显式 `/tmp` 放行、敏感目录限制和上级目录限制。
- `sandbox-jsonrpc-proxy`: 定义 agent 与 Bubblewrap helper 之间的 JSON-RPC 桥接协议，以及文件/进程操作如何通过 proxy 执行并把结果映射回宿主。

### Modified Capabilities

## Impact

- Affected code: `src/agent/sandbox.rs`、`src/agent/worker.rs`、`src/agent/runtime.rs`、`src/config.rs`、新增的 JSON-RPC proxy/helper 入口以及相关测试。
- Affected systems: shell 工具、文件类工具、后续需要进入统一执行层的 tool-owned process，尤其是 Bubblewrap 内的工作区文件同步链路。
- Runtime impact: Linux 上可配置为 Bubblewrap 沙箱模式；Docker 后端在接口层保留但当前显式未实现；默认同步目录为当前工作区根 `.`，且显式 `/tmp` 可用。
- Validation impact: 需要新增验收测试，验证通过 JSON-RPC 修改沙箱内允许路径的文件后，宿主机同路径可直接看到变更结果。
