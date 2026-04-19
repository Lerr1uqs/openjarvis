# Sandbox

## 定位

- `sandbox` 是 Agent 执行隔离层，对 `AgentWorker` 提供统一 `Sandbox` 抽象。
- 当前已落地 Bubblewrap 后端，并通过内部 JSON-RPC proxy 桥接沙箱与宿主机。
- 同时保留 `disabled / docker` 后端位形，其中 Docker 目前仍是保留入口。

## 边界

- 负责加载当前 workspace 的全局 sandbox capability 配置，并据此选择后端。
- 负责维护沙箱生命周期、默认同步目录映射，以及宿主路径访问校验。
- 负责对宿主敏感目录和上级目录逃逸做限制。
- 不负责 LLM 调度、不负责 tool 协议编排、不负责完整容器平台抽象。
- Docker 后端当前只保留 trait 对齐和后端选择入口，不承担实际运行职责。

## 关键概念

- `Sandbox`
  Worker 持有的统一沙箱接口，当前对外暴露后端标识、workspace 根目录和基础文件读写原语。
- `SandboxCapabilityConfig`
  从 `config/capabilities.yaml` 读取的全局 capability 策略，作用于当前 workspace 下的所有用户。
- `BubblewrapSandbox`
  真实 bwrap 后端，负责拉起隐藏 `internal-sandbox proxy` 并通过 JSON-RPC 执行文件原语。
- `workspace_sync_dir`
  默认 `.openjarvis/workspace`，是当前宿主机与沙箱共享可见的同步目录。

## 核心能力

- 按 capability 配置选择 `disabled / bubblewrap / docker` 后端。
- 在 Bubblewrap 模式下完成 proxy 拉起、握手、生命周期管理和基础文件读写。
- 限制 `/etc`、`/proc`、`/sys`、`/dev` 以及用户敏感目录等宿主路径访问。
- 默认拒绝通过 `..` 访问同步目录之外的上级路径。
- 保证通过 JSON-RPC 写入 `workspace_sync_dir` 的文件能被宿主机直接看到。
- Docker 后端当前明确返回未实现错误，避免误判为已具备隔离能力。

## 使用方式

- 主流程默认从当前 workspace 的 `config/capabilities.yaml` 加载策略并初始化 Worker 持有的 sandbox。
- 测试或嵌入式场景可以显式注入 `SandboxCapabilityConfig`。
- 真实安全边界应建立在 `Sandbox` 层上，不要绕过该层直接访问宿主敏感路径。
