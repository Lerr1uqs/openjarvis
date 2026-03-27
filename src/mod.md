# src 模块总览

## 作用

`src/` 是 OpenJarvis 的主运行时入口层，负责把外部消息接进来，整理成统一上下文，交给 Agent 执行，再把结果发回外部渠道。

如果以后只想快速判断“问题大概在哪一层”，优先读这个文件，不要直接从源码全文开始扫。

## 根层子模块

- `agent`
  Agent 执行层。负责 ReAct 循环、工具调用、运行时注册表、hook、worker 编排。
- `channels`
  外部接入层。负责和飞书等上游平台建立连接，收消息、发消息。
- `cli.rs`
  命令行入口定义。负责解析主命令、内部 MCP 命令、内部浏览器 sidecar 命令。
- `command.rs`
  斜杠命令预处理层。负责在消息进入 Session 和 Agent 之前拦截 `/xxx` 命令。
- `compact`
  上下文压缩层。负责在上下文过大时，把旧 chat 历史折叠成更小的替代 turn。
- `config.rs`
  配置加载层。负责应用配置、Agent 配置、Tool 配置、MCP 配置、Hook 配置、LLM 配置。
- `context`
  统一消息上下文层。负责 `system` / `memory` / `chat` 三段式上下文组织。
- `llm.rs`
  LLM 适配层。负责统一请求结构，以及 OpenAI / Anthropic 协议序列化。
- `model.rs`
  外部消息模型层。负责 channel 与 router 之间共享的入站/出站消息结构。
- `router.rs`
  路由层。负责在 Channel、Command、Session、Agent 之间做事件编排与转发。
- `session.rs`
  会话存储层。负责按用户与线程定位上下文，并持久化线程级历史的内存快照。
- `thread.rs`
  线程历史层。负责描述 Thread、Turn、ToolEvent 这些对话持久化对象。
- `main.rs`
  二进制启动入口。负责装配配置、初始化日志、启动 channel 与 router。

## 核心概念

- `IncomingMessage`
  从外部渠道收到的一条原始消息，是系统的外部输入单位。
- `OutgoingMessage`
  准备发回外部渠道的一条消息，是系统的外部输出单位。
- `Session`
  同一 `channel + user_id` 下的长期会话空间。可以理解为“这个用户在这个渠道上的总上下文盒子”。
- `Thread`
  `Session` 内的一条连续任务线。一个用户可能同时有多条 thread。
- `Turn`
  一次用户输入触发的一轮处理结果，通常包含 user / assistant / tool_call / tool_result 等一组消息。
- `MessageContext`
  给 LLM 的标准化上下文容器，拆成 `system`、`memory`、`chat` 三段。
- `AgentWorker`
  长生命周期执行体。负责接收路由层请求，并把单轮任务交给 `AgentLoop`。
- `AgentLoop`
  单轮 ReAct 执行器。负责调 LLM、调用工具、汇总最终输出。
- `ToolRegistry`
  工具注册中心。负责维护当前线程可见的工具、工具集、MCP 工具与技能加载。
- `Compact`
  线程级 chat 历史压缩机制。目标不是“摘要存档”，而是“保留继续工作所需的最小上下文”。

## 备注

- `src/bin/` 当前是二进制扩展入口目录，不是库模块；目前没有实际源码文件，因此这里不单独建模块说明。
