# Channel

## 定位

- `Channel` 是外部聊天平台和 OpenJarvis 之间的协议适配层。
- 它把平台事件收敛成统一的 `IncomingMessage`，再把统一的 `OutgoingMessage` 发回平台。

## 边界

- 负责连接、收消息、发消息、健康检查。
- 平台侧必要的入站预处理能力也属于这里，例如签名校验、平台专用的内存去重前置层。
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
- `FeishuMemoryDeduper`
  `Feishu` 入站侧的内存 TTL 去重层，只做单进程 best-effort 去重，不进入 `Session/Thread` 持久化模型。

## 核心能力

- 维持平台长连接或事件循环。
- 把平台字段映射为统一消息模型。
- 在进入主链路前执行平台专用的轻量预处理，例如 `Feishu` 的内存 TTL 去重。
- 根据 `OutgoingMessage.target` 把结果发送回正确会话。

## 验收标准

- channel 只做平台协议适配和平台前置处理，不把平台 dedup 状态写入 `Session/Thread`。
- `FeishuMemoryDeduper` 失效、过期或进程重启后，同一消息可能再次进入主链路；这是显式接受的副作用边界，不属于 `Session/Thread` 职责。

## 使用方式

- 新增平台时，实现 `Channel` trait。
- Router 负责注册并启动 channel；channel 本身不感知 Agent 内部状态。
