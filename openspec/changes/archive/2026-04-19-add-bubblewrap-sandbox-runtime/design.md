## Context

当前仓库的 `src/agent/sandbox.rs` 仍然只提供 `DummySandboxContainer`，`AgentWorker` 只是持有一个占位对象，实际文件访问和工具自有子进程仍然直接运行在宿主机。与此同时，仓库已经形成了较稳定的内部 helper 形态：

- 顶层隐藏子命令通过 `src/cli.rs` 和 `src/cli_command/internal.rs` 统一调度
- browser sidecar 通过内部命令和结构化协议复用长期运行进程
- `ToolRegistry`、`AgentRuntime`、`AgentWorker` 已经把大多数副作用入口收口到少量模块

这意味着沙箱接入不需要推翻现有线程/工具模型，而是应该沿用“worker 持有 runtime 资源、内部 helper 通过隐藏命令启动、上层模块依赖抽象接口”的已有结构。

本次还有三个额外约束：

- 需要保留 Docker 作为并列后端能力，但当前允许显式 `unimplemented!()`
- 需要新增面向全体用户的 capability 配置文件 `config/capabilities.yaml`
- 验收必须证明：通过 JSON-RPC 在沙箱允许路径中修改文件后，宿主机同路径能直接看到结果

## Goals / Non-Goals

**Goals:**

- 定义统一 `Sandbox` trait，并让 `AgentWorker` 持有真实的沙箱抽象而不是占位对象
- 实现 Bubblewrap 后端，采用长期运行的 JSON-RPC proxy/helper 处理沙箱内文件与进程操作
- 在配置层新增 `config/capabilities.yaml`，声明默认沙箱后端、默认同步目录、敏感目录限制和上级目录访问限制
- 让默认同步目录固定为当前工作区根 `.`，同时允许显式 `/tmp/...` 绝对路径
- 保留 Docker 后端类型与初始化入口，但当前显式返回未实现错误
- 提供可验证的文件同步闭环测试，证明宿主和沙箱共享工作区写入结果

**Non-Goals:**

- 本次不修改 `model/*.md` 组件文档
- 本次不把所有现有工具一次性全部切到沙箱后端，先完成最小可验证的文件与命令原语闭环
- 本次不实现完整 Docker runtime，也不接入 OCI 编排
- 本次不做跨平台等价支持，Bubblewrap 后端只面向 Linux

## Decisions

### 1. 以 `Sandbox` trait + `SandboxBackendKind` 建模后端，而不是继续扩展 `DummySandboxContainer`

新增统一 trait，例如：

- `kind()`
- `capabilities()`
- `workspace_root()`
- `start()`
- `stop()`
- `send_jsonrpc(request)`

并用 `SandboxBackendKind::{Bubblewrap, Docker}` 建模后端种类。`AgentWorker` 只依赖 trait，不直接依赖 Bubblewrap 细节。

Why:

- 能把 Bubblewrap 与 Docker 的差异隔离在后端实现里
- 让 `AgentWorker` 与后续工具执行层只关心“有一个沙箱会话可调用”
- 避免未来再经历一次从占位 struct 迁到 trait 的横切改造

Alternative considered:

- 继续扩展 `DummySandboxContainer`，用 `enum` 塞所有状态。
  Rejected，因为这会让占位类型逐步膨胀成多职责对象，测试与后续扩展都更差。

### 2. Bubblewrap 采用隐藏 `internal-sandbox proxy` 子命令 + JSON-RPC 2.0 over stdio

Bubblewrap 后端不直接在 Rust 主进程里做文件访问，而是：

1. Host 侧启动 `bwrap ... openjarvis internal-sandbox proxy ...`
2. proxy 进程在沙箱内运行，通过 stdin/stdout 收发 JSON-RPC 2.0 消息
3. Host 侧 `BubblewrapSandbox` 保存该 proxy 句柄，并通过请求 ID 做同步调用

Why:

- 复用现有 internal helper 模式，与 browser/internal-mcp 一致
- 结构化协议比 ad-hoc stdin 文本更易测试和扩展
- 后续新增更多文件/进程原语时不需要重新设计宿主到沙箱的桥

Alternative considered:

- 每次文件操作都重新 `bwrap` 一次并通过 CLI 参数传动作。
  Rejected，因为生命周期成本高，且很难扩展到多步操作和长期会话。

### 3. `config/capabilities.yaml` 作为独立全局配置文件，不塞回 `config.yaml`

新增独立文件 `config/capabilities.yaml`，由启动时统一加载。建议结构围绕“全体用户共享的默认能力”展开，例如：

- `sandbox.backend`
- `sandbox.workspace_sync_dir`
- `sandbox.restricted_host_paths`
- `sandbox.allow_parent_access`
- `sandbox.bubblewrap.executable`

Why:

- 这是面向全体用户的 capability 策略，不属于单个 agent/tool 的局部运行参数
- 独立文件更容易单独演进和审计，不会把 `config.yaml` 变成杂糅配置中心
- 与用户提出的文件位置要求一致

Alternative considered:

- 放进 `agent.tool` 或 `agent.sandbox` 子段。
  Rejected，因为这会把“全局能力策略”混到单实例运行配置里，边界不清晰。

### 4. 默认同步工作区根 `.`，允许显式 `/tmp`，并显式拒绝上级目录穿透

Bubblewrap 默认可写映射目录为当前工作区根 `.`。此外，显式 `/tmp/...` 绝对路径会被允许并直接映射到宿主 `/tmp`。所有 JSON-RPC 文件路径在进入后端前都先规范化，并执行两层检查：

- 不允许通过相对路径逃逸出同步根目录
- 不允许通过 `..`、符号链接后的上级路径、或直接给绝对宿主路径来访问工作区父级目录

另外，为了避免 agent 语义混乱，host 与 proxy 间的路径编码需要保留 agent 在 sandbox 中看到的可复用路径语义：

- `/workspace/...` 这类 sandbox 内可见路径后续应可直接被 `read` / `write` / `edit` / `exec_command.workdir` 继续使用
- 显式 `/tmp/...` 路径也应保持原样传递，而不是被偷偷改写成某个 workspace 相对路径

Why:

- 用户已经明确要求把 `.` 作为默认同步目录，并允许 `/tmp`
- 即使同步根扩大到工作区根，仍然需要保留对上级目录逃逸和敏感目录的硬限制
- 宿主机可直接看到变更，因为这些路径本质上仍是 bind mount 到同一真实目录

Alternative considered:

- 仍然只同步一个独立子目录，而不是工作区根。
  Rejected，因为这会让 agent 在 sandbox 内观察到的路径和后续工具调用路径脱节，出现“`ls .` 看得到但不能直接 `cat`”的语义割裂。

### 5. 敏感目录限制在 host 和 sandbox 两侧都做，失败时显式报错

限制策略不只体现在 Bubblewrap mount 参数上，还要在 host 侧 path policy 先做校验。典型敏感目录包括：

- `~/.ssh`
- `~/.gnupg`
- `/etc`
- `/proc`
- `/sys`
- `/dev`

Host 侧先拒绝，Bubblewrap 侧再不挂载，形成双重防线。

Why:

- 只依赖 bwrap mount 配置不够可观察，错误信息也不够友好
- host 侧先拦截便于测试“为什么拒绝”

Alternative considered:

- 只依赖 Bubblewrap 不挂载敏感目录。
  Rejected，因为路径策略不可见，难以给 agent/测试明确失败语义。

### 6. Docker 后端保留 trait 实现，但当前显式未实现

本次在后端枚举、配置解析和工厂方法中保留 `Docker`，但是初始化路径直接返回 `unimplemented!()` 或等价明确错误。

Why:

- 满足“保留 docker 支持”的架构要求
- 避免在本次 change 中把范围扩展到第二个真正后端

Alternative considered:

- 暂时完全不声明 Docker。
  Rejected，因为这会让后端抽象一开始就带有 Bubblewrap 偏置。

## Risks / Trade-offs

- [沙箱 path policy 与真实挂载边界不一致] -> 先在 host 侧做统一规范化校验，再把同一组规则用于 Bubblewrap 参数生成。
- [JSON-RPC proxy 进程崩溃会让 worker 持有失效句柄] -> 对 proxy 调用增加健康检查与结构化错误，必要时让 worker 初始化失败而不是继续运行。
- [同步目录扩大到工作区根会增大写入面] -> 继续保留敏感目录限制、上级目录逃逸限制，并只额外允许显式 `/tmp` 作为临时目录。
- [Docker 后端未实现会让用户误以为可用] -> 在配置验证和初始化错误中明确指出当前仅 Bubblewrap 可用。
- [当前 model 文档仍写着 placeholder] -> 本次只在 OpenSpec 和代码里推进，最终向用户显式报告模型文档尚未同步。

## Migration Plan

1. 新增 `config/capabilities.yaml` 读取、校验与默认值解析。
2. 重写 `src/agent/sandbox.rs`，引入 trait、后端枚举、Bubblewrap/Docker 工厂和 JSON-RPC 类型。
3. 扩展 CLI 与 internal command registry，新增 `internal-sandbox proxy` 入口。
4. 让 `AgentWorker` 持有真实沙箱实例，并在启动时按 capability 配置初始化。
5. 先实现最小 JSON-RPC 文件原语与同步目录闭环测试。
6. 再补充敏感目录限制、上级目录限制、Docker 未实现错误和配置测试。

Rollback strategy:

- 直接切换 capability 配置到 `local/disabled` 模式，或者回退到当前占位实现
- internal-sandbox 命令与新配置文件均为增量引入，不会破坏既有 browser/internal-mcp helper

## Open Questions

- 首版是否需要同时把 shell 工具接到 JSON-RPC proxy，还是先完成文件闭环与能力配置闭环。
- 敏感目录默认列表是否需要允许用户追加，还是先完全固定。
- `/workspace/...` 和宿主工作区路径之间的映射是否还需要进一步对 agent 显式暴露更多说明。
