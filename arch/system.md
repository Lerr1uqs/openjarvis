# 整体架构

OpenJarvis 当前主链路是：

`Channel -> Router -> Session -> AgentWorker -> AgentLoop -> Router -> Channel`

## 设计原则

- `Thread` 是唯一线程级持久化聚合。
- 消息边界以 `ChatMessage` 为准，不再依赖旧兼容容器。
- `AgentLoop` 只处理 request-time working set，不负责线程初始化。
- `compact` 只接收 message 序列，不依赖 turn slicing。
- 全局目录和共享设施归 `AgentRuntime`，线程级真相归 `Thread`。

## Channel

- 外部输入点，负责和飞书、Telegram、CLI 等上游建立连接。
- 输入输出单位分别是 `IncomingMessage` 和 `OutgoingMessage`。
- Channel 不理解线程内部状态，也不直接操作 Session/Thread。

## Router

- Router 只负责事件编排、消息转发和组件连接。
- 它不直接执行 Agent 逻辑，不直接持有线程运行时工作集。
- Router 负责把入站消息交给 Session 解析线程，再把请求发送给 Worker。

## Session

- Session 是 `channel + user_id` 维度的长期会话空间。
- 一个 Session 下可以有多个 `Thread`。
- `thread_key = user_id:channel:external_thread_id`
- `internal thread id` 由 `thread_key` 稳定派生。

当前 Session 层职责：

- 缓存线程快照
- 从 store 懒加载 `Thread`
- 以 write-through 方式持久化线程快照
- 维护 `external_message_id` 去重记录

## Thread

```rust
pub struct Thread {
   pub locator: ThreadContextLocator,
   pub thread: ThreadContext,
   pub state: ThreadState,
}
```

边界如下：

- `ThreadContext` 只负责持久化消息域
- `ThreadState` 只负责 feature/tool/approval 等非消息状态
- request-time 临时消息只属于 `AgentLoop` 局部执行期
- `Turn` 只保留为提交概念，不再是主存储边界

消息模型如下：

- 稳定 `System` messages 在 `init_thread()` 时一次性注入并持久化
- 持久化消息统一存放在 `Thread.thread.messages`
- `Thread::load_messages()` 返回非 `System` 持久化消息
- 完整 LLM 请求消息由 `AgentLoop` 在运行时临时拼接

## AgentWorker

- Worker 是长生命周期执行体。
- 它接收 Router 请求，并在进入 loop 前执行 `init_thread()`。
- `init_thread()` 的职责是：
  - 准备 feature/tool registry
  - 生成稳定 system messages
  - 把稳定 system messages 写回 `Thread`
  - 如有改动，立即同步回 Session/store

## AgentLoop

- `AgentLoop` 是单轮 ReAct 执行器。
- 输入是“已经初始化好的 `Thread` + 当前 incoming message”。
- 它只维护本轮 request-time working set，不拥有线程初始化 ownership。

主职责：

- 基于 `Thread` 计算当前线程可见工具
- 调用 LLM
- 执行工具调用
- 触发 runtime compact
- 产出结构化事件和 commit messages
- 把跨轮状态回收到 `Thread`

## AgentRuntime

- `AgentRuntime` 是共享依赖容器。
- 当前只持有 `HookRegistry` 和 `ToolRegistry`。
- 它不保存线程级真相，不维护线程级 override 缓存。

## ToolRegistry

- `ToolRegistry` 是全局工具池和目录层。
- 它负责：
  - 注册 builtin tools
  - 注册 program-defined toolsets
  - 管理 MCP server 与 skill registry
  - 提供全局 tool / toolset / handler 解析入口

它不负责长期持有线程自己的：

- loaded toolsets
- tool visibility projection
- 线程级 feature override
- 工具权限与审批状态

这些 thread-scoped 状态统一由 `Thread` 管理。

## Compact

- `compact` 是线程消息压缩能力。
- 它的输入边界是 message 序列，不是 turn。
- 主链路只 compact 全部非 `System` message。
- `CompactManager` 只负责根据消息生成替代消息。
- 是否写回 `Thread` 由 `AgentLoop` 或其他调用方决定。

runtime compact 的输入是：

- `persisted non-system messages + pending live chat messages`

compact 写回规则是：

- 保留持久化 `System` 前缀
- 替换全部非 `System` 历史

## Context

- `context` 模块只定义统一消息协议。
- 它提供 `ChatMessage`、`ChatMessageRole`、`ChatToolCall` 和预算桶概念。
- 它不负责持久化，也不负责请求上下文拼装。

## Command

- Slash command 会在消息进入 Agent 前被截取处理。
- thread-scoped command 必须先 resolve 目标线程，再修改对应 `Thread` 状态。
- Command 不是第二套线程状态容器。

## LLMProvider

- 负责统一请求结构，以及 OpenAI / Anthropic 等协议适配。
- LLM 输入消息序列统一来自 `AgentLoop` 的运行时拼装结果。

## Context Budget

对最终送给 LLM 的完整请求做容量估算，而不是只看 chat。

容量估算至少拆成下面几部分：

- `system_tokens`
- `chat_tokens`
- `visible_tool_tokens`
- `reserved_output_tokens`
- `total_estimated_tokens`
- `context_window_tokens`
- `utilization_ratio`

这里的 `visible_tool_tokens` 只统计当前线程当前时刻真正对模型可见的工具，不统计已经注册但当前不可见的工具。

## Auto Compact

- `auto_compact` 是基于 `compact` 的可选增强能力。
- 开启后，loop 会在 generate 前给模型注入当前上下文容量提示，并让 `compact` 工具可见。
- 不开启时，runtime 仍然可以在阈值达到时执行被动 compact。
