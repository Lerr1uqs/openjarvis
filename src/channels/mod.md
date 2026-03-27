# channels 模块总览

## 作用

`channels/` 是系统的外部接入层，负责和具体消息平台打交道。对内它只暴露统一的 `IncomingMessage` / `OutgoingMessage` 通道，对外则适配每个平台自己的协议和发送方式。

## 子模块

- `feishu.rs`
  飞书渠道适配器。负责飞书消息接入、文本提取、回复发送、鉴权与长连接协作。

## 核心概念

- `Channel`
  外部渠道抽象接口。所有平台接入都要实现同样的启动、健康检查、收发约定。
- `ChannelRegistration`
  Router 分配给某个渠道的一对通信通道，用于把入站消息交给 Router，把出站消息发回渠道。
- `IncomingMessage`
  渠道适配器统一后的上行消息模型。
- `OutgoingMessage`
  Router 或 Agent 生成后，准备下发到某个渠道的消息模型。

## 设计意图

- 外部平台差异应尽量收敛在本模块，不向 Router 和 Agent 泄漏平台细节。
- Router 理想上只需要知道“这是哪个 channel 发来的消息”，不需要知道飞书 HTTP 字段长什么样。

## 扩展方式

- 后续新增 Telegram、Discord、Slack 等渠道时，应继续在这里新增适配器，而不是把平台分支逻辑塞回 Router。
