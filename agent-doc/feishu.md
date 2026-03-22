# Feishu 集成说明

## 当前目标

当前项目只实现一条最小闭环：

`Feishu -> message -> router -> agent -> react loop -> llm/tool -> message -> Feishu`

其中：

- 入站使用 Feishu 长连接
- 出站使用 Feishu 服务端发送消息 API
- 额外会先给用户原消息添加一个 `Typing` reaction
- agent loop 里的 `tool_call / tool_result` 事件也会直接回到当前群聊

## 当前实现结构

当前代码不是纯 Rust 直连 Feishu 长连接，而是拆成两层：

1. Rust 主程序
   - 负责 `router / agent / llm / Feishu 发消息`
2. Node sidecar
   - 使用 Feishu 官方 Node SDK 建立长连接
   - 接收 `im.message.receive_v1`
   - 通过 `stdout` 把标准化事件推给 Rust

原因很简单：

- Feishu 长连接官方 SDK 成熟，接入快
- 当前项目目标是先跑通链路，不优先重写私有协议
- Rust 继续保留核心路由和 agent 逻辑

如果要看整体模块关系和统一调用链，额外参考：

- [agent-doc/architecture.md](F:/coding-workspace/openjarvis/agent-doc/architecture.md)
- [agent-doc/context-session-thread.md](F:/coding-workspace/openjarvis/agent-doc/context-session-thread.md)

## 用到的飞书概念

### 1. 企业自建应用

长连接模式只支持企业自建应用，不支持商店应用。

当前配置依赖：

- `app_id`
- `app_secret`

## 2. 机器人能力

如果没有开启机器人能力：

- 不能正常接收机器人相关消息事件
- 不能调用发消息接口
- 不能调用 reaction 接口

## 3. 长连接模式

当前项目默认使用 `long_connection` 模式。

它的特点是：

- 本地机器直接连接 Feishu WebSocket
- 不需要公网 webhook
- 不需要内网穿透
- 飞书后台必须把事件订阅方式切成“使用长连接接收事件”

## 4. 接收消息事件

当前只处理：

- `im.message.receive_v1`

当前只取这几个字段：

- `sender.sender_id.open_id`
- `sender.sender_type`
- `sender.tenant_key`
- `message.message_id`
- `message.chat_id`
- `message.thread_id`
- `message.chat_type`
- `message.message_type`
- `message.content`

当前只支持文本消息，非文本会降级成占位文本。

## 5. tenant_access_token

Rust 出站调用 Feishu 服务端 API 时，先通过：

- `auth/v3/tenant_access_token/internal`

获取 `tenant_access_token`，然后再调用消息相关 API。

当前实现里做了简单缓存，避免每次消息都重新换 token。

## 6. 发送消息

当前文本回复使用：

- `POST /open-apis/im/v1/messages`

发送目标当前统一使用：

- `receive_id_type=chat_id`

也就是直接回复到原消息所在会话。

## 7. 消息表情回复 reaction

当前会先对用户原消息添加一个 reaction，再发文本。

使用接口：

- `POST /open-apis/im/v1/messages/:message_id/reactions`

当前使用的表情类型：

- `Typing`

它对应飞书文案里的“敲键盘”。

## 当前项目里的 Feishu 相关代码

### Rust

- [src/main.rs](F:/coding-workspace/openjarvis/src/main.rs)
  - 启动主进程
  - 加载配置并批量注册 channels

- [src/channels/feishu.rs](F:/coding-workspace/openjarvis/src/channels/feishu.rs)
  - Feishu channel 实现
  - 统一入站消息转换
  - 获取 `tenant_access_token`
  - 添加 reaction
  - 发送文本消息

- [src/model.rs](F:/coding-workspace/openjarvis/src/model.rs)
  - `IncomingMessage`
  - `OutgoingMessage`

- [src/router.rs](F:/coding-workspace/openjarvis/src/router.rs)
  - `ChannelRouter`
  - 根据 `ChannelConfig` 注册全部 channels
  - 入站去重和出站派发

### Node

- [scripts/feishu_ws_client.mjs](F:/coding-workspace/openjarvis/scripts/feishu_ws_client.mjs)
  - 使用官方 `@larksuiteoapi/node-sdk`
  - 建立长连接
  - 监听 `im.message.receive_v1`
  - 通过 `stdout` 输出 JSON 事件

- [package.json](F:/coding-workspace/openjarvis/package.json)
  - 管理 sidecar 依赖

## 当前配置项

配置文件见 [config.yaml](F:/coding-workspace/openjarvis/config.yaml)。

当前 Feishu 相关配置：

- `feishu.mode`
  - 当前使用 `long_connection`
- `feishu.webhook_path`
  - 当前配置里保留，但这版运行时未实现 webhook 模式
- `feishu.open_base_url`
  - 默认 `https://open.feishu.cn`
- `feishu.app_id`
- `feishu.app_secret`
- `feishu.verification_token`
  - 当前 webhook 备用
- `feishu.encrypt_key`
  - 当前 webhook 加密未实现
- `feishu.dry_run`
  - `false` 时会真实调用 Feishu 发消息
- `feishu.auto_start_sidecar`
  - 是否自动拉起 Node 长连接 sidecar
- `feishu.node_bin`
- `feishu.sidecar_script`

## 当前技术栈

### Rust 侧

- `tokio`
  - 异步运行时
- `reqwest`
  - 调用 Feishu 服务端 API
- `serde / serde_json / serde_yaml`
  - 配置与消息序列化
- `tracing`
  - 日志

### Node 侧

- `@larksuiteoapi/node-sdk`
  - Feishu 官方 SDK
  - 用于长连接接收事件

## 当前限制

- 只处理 `im.message.receive_v1`
- 只支持文本消息
- 只发送文本消息
- reaction 固定写死为 `Typing`
- 当前支持 mock 和 OpenAI-compatible/deepseek
- 当前 ReAct 只支持一轮工具调用
- webhook 加密回调未实现
- 长连接是 sidecar 方案，不是 Rust 原生实现
- 当前这版运行时未实现 webhook 模式

## 参考文档

- 长连接接收事件
  - https://open.feishu.cn/document/server-docs/event-subscription-guide/event-subscription-configure-/request-url-configuration-case
- 接收消息
  - https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message/events/receive
- 发送消息
  - https://open.feishu.cn/document/server-docs/im-v1/message/create
- 获取 tenant_access_token
  - https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal
- 添加消息表情回复
  - https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/create
- 表情文案说明
  - https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/emojis-introduce
