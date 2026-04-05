# Agent 子模块

`agent/` 内部再分成两条线：

- 执行线：`worker -> loop -> runtime`
- 能力线：`hook` 和 `tool/*`

文档索引：

- `worker.md`
  Agent 请求入口和长生命周期执行体。
- `loop.md`
  单轮 ReAct 执行循环。
- `runtime.md`
  hooks、tools、compact runtime 的共享容器。
- `hook.md`
  生命周期观测与脚本扩展。
- `sandbox.md`
  执行环境占位模型。
- `tool.md`
  工具总目录模型。
- `tool/toolset.md`
  渐进式工具集模型。
- `tool/browser.md`
  浏览器工具集模型。
- `tool/command-session-manual.md`
  命令会话手工验收入口。
- `tool/mcp.md`
  MCP 工具托管模型。
- `tool/skill.md`
  本地 skill 加载模型。
