# agent 模块总览

## 作用

`agent/` 是系统的执行层，负责把“已经路由好的一个任务”真正跑完。这里不关心外部渠道细节，核心关注的是：上下文、LLM、工具、hook、单轮任务执行。

## 子模块

- `agent_loop.rs`
  单轮 ReAct 执行引擎。负责请求 LLM、解析 tool call、执行工具、回填 tool result、生成最终 assistant 输出。
- `hook.rs`
  生命周期观察层。负责在关键事件点触发外部脚本或其他 hook 处理器。
- `runtime.rs`
  运行时容器。负责把 hooks 和 tools 组合成一个 Agent 可复用的共享运行时。
- `sandbox.rs`
  沙箱占位层。当前是占位实现，用来表达“Worker 将来会持有一个真实沙箱容器”。
- `tool/`
  工具子系统。负责工具定义、注册、渐进式加载、MCP 集成、浏览器工具、skill 工具等。
- `worker.rs`
  Worker 编排层。负责接收请求、装配上下文、调用 `AgentLoop`，并把结果回传给 Router。

## 核心概念

- `AgentWorker`
  长生命周期的 Agent 执行单元。它像一个“后台工人”，从输入队列里拿任务，并串行跑完。
- `AgentRequest`
  Router 发给 Worker 的任务请求，表示“请处理这个线程上的这一轮输入”。
- `AgentLoop`
  真正执行单轮对话逻辑的核心对象。它只关注这轮任务如何完成，不负责长期队列管理。
- `AgentRuntime`
  运行时依赖容器。可以理解为 Agent 的“共享能力包”，里面放 hook 注册表和 tool 注册表。
- `Hook`
  Agent 生命周期上的观测点或扩展点，不改变主流程抽象，但能附加外部行为。
- `Sandbox`
  执行环境抽象。当前只是占位，未来会承接真正隔离执行能力。

## 边界

- `agent/` 不负责和飞书之类平台通信，那是 `channels/` 的职责。
- `agent/` 不负责决定消息属于哪个 Session / Thread，那是 `router.rs` 和 `session.rs` 的职责。
- `agent/` 不直接定义外部消息模型，那是 `model.rs` 的职责。
