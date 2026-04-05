## Context

当前仓库已经完成了 `ThreadContext -> ToolRegistry -> ToolHandler` 这一层的线程级工具调用收口，但执行边界仍然是散的：

- `ToolRegistry` 只负责 handler 查找和调用，本身没有执行层概念。
- builtin `read/write/edit` 在 Rust 主进程内直接访问宿主文件系统。
- builtin `bash` 直接在宿主环境启动 `sh/powershell`。
- browser sidecar 和 stdio MCP server 也直接在宿主环境启动子进程。
- `sandbox` 模块仍然只是 `DummySandboxContainer` 占位，不承担真实隔离职责。

Bubblewrap 的能力边界也很明确：它只能隔离“由它启动的子进程”，不能反向限制当前 Rust 主进程里已经发生的 `std::fs` 访问。这意味着如果要让“所有工具执行层”真正支持沙箱，方案不能停留在给 `bash` 外面套一层 `bwrap ... sh -lc`，而必须把文件访问和工具自有子进程启动都迁到统一执行层之后，再由本地或沙箱后端承接。

## Goals / Non-Goals

**Goals:**

- 在工具子系统中引入统一执行层，并把执行环境明确建模为 `Local` / `Sandbox` 两种。
- 保持 `ToolRegistry` 的目录层职责不变，让工具执行方式成为独立 runtime concern。
- 用最小但完整的原语承接工具副作用，至少覆盖文件读写和子进程启动。
- 在 Linux 上使用 Bubblewrap 作为 `Sandbox` 的首个实现，并通过长期存活的 helper 进程复用沙箱。
- 将 builtin 文件工具、`bash`、memory、browser sidecar 和 stdio MCP server 纳入执行层迁移范围。
- 在配置错误或平台不支持时显式失败，避免静默退回本地执行。

**Non-Goals:**

- 本次不把 hooks、Feishu sidecar 或其他非工具执行链路一并纳入沙箱。
- 本次不设计通用 OCI 容器后端，也不把 `Sandbox` 泛化成多种 provider 的插件市场。
- 本次不承诺 Windows/macOS 上提供等价沙箱能力；非 Linux 平台继续只有 `Local` 可用。
- 本次不修改 `model/**` 架构文档，只在 OpenSpec 里定义方案和实现任务。

## Decisions

### 1. 执行层作为 `AgentRuntime` 的共享依赖存在，而不是塞回 `ToolRegistry`

执行层的职责是“怎么做副作用”，而不是“这个工具是否可见、由谁处理”。因此它应该像 hooks、tools 一样作为 runtime 依赖被注入，而不是让 `ToolRegistry` 同时承担目录层和执行容器所有权。推荐形态：

- `AgentRuntime` 持有 `ToolExecutionRuntime`
- `ToolRegistry` 在注册 builtin/toolset handler 时把 runtime 传给需要副作用的实现
- `ToolHandler` 继续保留统一对外接口，但内部通过 runtime 调用文件/进程原语

这样可以保持当前线程状态和工具目录边界不变，只把“副作用如何落地”独立出来。

Alternative considered:
- 让 `ToolRegistry` 自己拥有 Bubblewrap 容器和所有执行逻辑。
  Rejected，因为这会重新把 `ToolRegistry` 变成跨职责的大对象，和当前“目录层 / runtime concern 分离”的方向相冲突。

### 2. 不为每个工具写两套实现，而是只为底层原语提供本地与沙箱两套 backend

上层工具不应该各自维护一套 host 逻辑和一套 sandbox 逻辑。执行层只暴露少量原语，例如：

- `read_text`
- `write_text`
- `replace_text`
- `spawn_process`
- `wait_process`
- `kill_process`

具体工具共享原有参数解析、结果包装和业务语义，只把“怎么读写文件”“怎么启动/回收进程”交给 backend。这样可以避免工具级双写，并把 Local/Sandbox 差异压缩到最小边界。

Alternative considered:
- 每个 tool handler 自己判断当前是本地还是沙箱，然后分支执行。
  Rejected，因为这会让差异散落在所有工具里，无法形成统一安全边界，也会显著放大测试成本。

### 3. Bubblewrap 后端采用“长期存活 helper + 结构化协议”，而不是“每次工具调用都单独 bwrap 一次”

对 `bash` 这类一次性命令来说，每次 `bwrap` 一次表面上可行；但 browser sidecar、stdio MCP server 和未来的长生命周期工具状态都不适合这样做。推荐方案是：

- Host 侧 runtime 启动一次 `bwrap ... openjarvis internal-tool-runtime-helper`
- helper 运行在沙箱内，通过 stdin/stdout 接收 JSON Lines 请求
- 后续文件操作和子进程操作都转发给 helper 执行

这样可以复用沙箱生命周期，也能统一管理 tool-owned process handle。

Alternative considered:
- 只给 `bash` 外层包 `bwrap`，其他工具暂时不处理。
  Rejected，因为这无法满足“包装所有工具执行层”的目标，`read/write/edit/memory` 仍然会直接访问宿主文件系统。

### 4. 沙箱 helper 只承接通用执行原语，上层工具不直接感知 Bubblewrap

helper 协议应该围绕执行原语而不是具体工具名设计。也就是说，helper 只知道“读文件、写文件、替换文本、启动进程、等待进程、杀进程”，不知道 `bash`、`browser__navigate`、`memory_write` 这些概念。好处是：

- 上层工具保持 tool 语义与结果包装稳定
- helper 可以复用于 builtin tool、memory、browser sidecar、stdio MCP
- 后续如果替换为其他沙箱后端，只需要重写 runtime/helper 这一层

Alternative considered:
- 让 helper 直接实现 `bash`、`read`、`write` 等具体工具协议。
  Rejected，因为这会把 tool 语义耦合进沙箱 backend，未来更换 backend 或调整 tool schema 都会放大改动面。

### 5. Bubblewrap 沙箱边界采用“显式挂载 + 路径映射 + 默认清空环境”，网络默认保留但可配置

考虑到 browser toolset 和部分 stdio MCP server 需要联网，首版设计建议：

- 默认使用 `--clearenv`
- 默认显式挂载运行 helper 所需只读系统路径
- 将工作区映射到沙箱内固定挂载点，例如 `/workspace`
- 默认保留网络能力，使用 `--share-net`
- 后续可通过配置关闭网络共享

这样首版仍然能获得文件系统与进程边界控制，同时不把 browser/MCP 直接做残。

Alternative considered:
- 默认彻底隔离网络。
  Rejected，因为这会让 browser 和大量依赖联网的工具开箱即不可用，首版落地成本过高。

### 6. `Sandbox` 配置失败时显式报错，不允许静默回退到 `Local`

如果用户明确选择了 `Sandbox`，静默回退到 `Local` 会制造虚假的安全边界。正确行为应该是：

- runtime 初始化检查当前平台、`bwrap` 可执行文件、helper 启动能力
- 任一前置条件失败时直接返回明确错误
- 只有用户显式配置 `Local` 时才使用宿主执行

Alternative considered:
- `Sandbox` 不可用时自动退回本地执行并打印 warning。
  Rejected，因为这会让配置语义不可信，尤其在安全相关场景下风险不可接受。

### 7. 迁移范围限定为“工具执行层”，不扩展到非工具辅助进程

本次变更的迁移范围包括：

- builtin `read/write/edit/bash`
- memory toolset 的文件仓库访问
- browser toolset 的 sidecar 启动
- stdio MCP server 的进程启动

不包括：

- hook command
- Feishu long connection sidecar
- 其他不属于 tool subsystem 的进程

这样才能与当前 change 的目标对齐，避免范围失控。

## Risks / Trade-offs

- [这是一次跨 builtin tools、memory、browser、MCP 的横切改造] -> 先收口原语，再分批迁移具体工具，避免一步到位重写全部模块。
- [长期 helper 进程可能崩溃或协议失步] -> runtime 需要加入健康检查、结构化错误和必要的重启策略。
- [Bubblewrap 挂载策略不当会导致工具可用性下降] -> 把网络、只读挂载、额外挂载点做成显式配置，并先围绕 workspace 最小闭环设计。
- [memory/browser/MCP 的迁移量比只改 `bash` 大很多] -> 这正是方案真实性的代价；如果不迁这些路径，就无法宣称“所有工具执行层都已统一”。
- [Linux-only 沙箱能力会带来跨平台差异] -> 将 `Local` 保留为默认和跨平台后备模式，在 spec 中明确 `Sandbox` 首期只针对 Linux + Bubblewrap。

## Migration Plan

1. 在配置层增加工具执行环境配置，并把 `AgentRuntime` 扩展为同时持有 `ToolExecutionRuntime`。
2. 先实现 `Local` backend，使所有目标工具都能通过统一原语运行但保持现有行为。
3. 新增隐藏 helper 入口和 Bubblewrap backend，完成 helper 生命周期、协议、挂载和路径映射。
4. 迁移 builtin 文件工具、`bash`、memory、browser sidecar、stdio MCP server 到执行层。
5. 为本地和沙箱两种模式补充单元测试与 Linux 集成验证。

Rollback strategy:

- 配置层可直接切回 `Local` 继续运行现有宿主语义。
- 如果需要整体回滚，实现阶段只需移除 execution runtime 注入和 helper 入口，不影响 thread/toolset 模型本身。

## Open Questions

- sandbox 默认允许哪些额外挂载点，例如缓存目录、浏览器下载目录或 `.openjarvis/memory` 之外的持久化路径。
- browser toolset 的下载、截图和临时产物是否全部限定在 workspace 内部，还是需要单独的可写沙箱目录。
- stdio MCP server 如果自身再启动子进程，是否需要额外约束，还是先把责任限定在“OpenJarvis 直接启动的那一层”。
