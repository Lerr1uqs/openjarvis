# Channel

## 定位

- `Channel` 是外部聊天平台和 OpenJarvis 之间的协议适配层。
- 它把平台事件收敛成统一的 `IncomingMessage`，再把统一的 `OutgoingMessage` 发回平台。

## 边界

- 负责连接、收消息、发消息、健康检查。
- 不负责线程解析、会话持久化、命令执行、LLM 调用。

## 关键概念

- `Channel`
  统一 trait，约束 `name / on_start / start / check_health`。
- `ChannelRegistration`
  Router 分配给 channel 的双向通道。
- `IncomingMessage`
  平台入站消息的统一形态。
- `OutgoingMessage`
  系统回发消息的统一形态。

## 核心能力

- 维持平台长连接或事件循环。
- 把平台字段映射为统一消息模型。
- 根据 `OutgoingMessage.target` 把结果发送回正确会话。

## 使用方式

- 新增平台时，实现 `Channel` trait。
- Router 负责注册并启动 channel；channel 本身不感知 Agent 内部状态。
