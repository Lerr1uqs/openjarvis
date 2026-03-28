# AgentRuntime

## 定位

- `AgentRuntime` 是 Agent 的共享依赖容器。
- 它的职责不是保存线程真相，而是把执行期会复用的全局能力打包给 Worker 和 Loop。

## 边界

- 负责持有 `HookRegistry`、`ToolRegistry`、`CompactRuntimeManager`。
- 不负责消息历史，不负责线程级 loaded toolsets 的最终真相，不负责会话持久化。

## 关键概念

- `hooks`
  Agent 生命周期扩展点。
- `tools`
  全局工具目录和外部能力接入点。
- `compact_runtime`
  compact 兼容态 override 管理器。

## 核心能力

- 从配置构造 hooks 和 tools。
- 为多个请求复用同一组 registry。
- 作为 Worker 和 Loop 的共享能力入口。

## 使用方式

- Runtime 适合放全局目录和共享设施。
- 线程级动态状态仍应回收进 `ThreadContext`，Runtime 不应成为第二份线程状态源。
