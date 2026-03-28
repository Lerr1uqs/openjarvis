# Command

## 定位

- `CommandRegistry` 是消息进入 Agent 之前的硬前置处理层。
- 命令是线程级控制面，不是自然语言对话的一部分。

## 边界

- 负责识别 `/xxx` 消息、解析参数、调用命令处理器、返回标准化回复。
- 命令消息本身不进入会话历史，但命令对 `ThreadContext` 的修改会被持久化。
- 不负责自然语言理解，不经过 AgentLoop。

## 关键概念

- `CommandInvocation`
  一次命令调用的解析结果。
- `CommandHandler`
  命令执行接口，直接操作 `ThreadContext`。
- `CommandReply`
  统一返回格式，固定输出 `[Command][name][SUCCESS/FAILED]: ...`。

## 核心能力

- 在 Router 中抢先拦截命令。
- 基于已解析线程执行线程级状态修改。
- 当前内建命令包括测试命令、`clear`、`auto-compact`。

## 使用方式

- 新命令应注册到 `CommandRegistry`，并以 `ThreadContext` 为操作对象。
- 适合放这里的是线程控制命令，不适合放这里的是需要模型推理的能力。
