# OpenJarvis MVP

当前仓库已经接入一条最小消息链路：

`Feishu long connection -> ChannelRouter -> AgentWorker -> AgentLoop(ReAct 1 round) -> LLMProvider / ToolRegistry -> ChannelRouter -> Feishu sender`

当前阶段的实现范围：

- 已实现 Feishu 长连接接收 `im.message.receive_v1`
- 已实现 Feishu `im.message.receive_v1` 文本消息接收
- 已实现统一 `IncomingMessage / OutgoingMessage`
- 已实现 `ChannelRouter`
- 已实现 `AgentWorker`
- 已实现单轮 ReAct loop
- 已实现四个内置工具
  - `read`
  - `write`
  - `edit`
  - `shell`
- 已实现两类 LLM provider
  - `mock`
  - `openai_compatible` 及其别名 `openai` / `deepseek`
- 已实现 Feishu 文本消息回发
- 已实现 tool call / tool result 事件回发到当前群聊
- `memory / sandbox` 仍然为空壳，暂未接入

## 本地启动

1. 复制配置文件

```powershell
Copy-Item config.example.yaml config.yaml
```

2. 安装 Node sidecar 依赖

```powershell
npm install
```

3. 启动

```powershell
cargo run
```

默认监听地址：

- 当前长连接模式下不需要 HTTP 入口
- 默认会自动拉起一个 Node sidecar，通过飞书官方 SDK 建立长连接
- 启动时会按配置批量注册全部 channel；当前内置实现只有 `feishu`
- `AppConfig` 在代码层已拆成 `server / channels / llm` 只读子配置，启动层只把对应子配置传给对应模块

## 配置说明

`config.yaml`

```yaml
server:
  bind: "0.0.0.0:3000"

feishu:
  mode: "long_connection"
  webhook_path: "/webhook/feishu"
  open_base_url: "https://open.feishu.cn"
  app_id: "cli_xxx"
  app_secret: "xxx"
  verification_token: ""
  encrypt_key: ""
  dry_run: true
  auto_start_sidecar: true
  node_bin: "node"
  sidecar_script: "scripts/feishu_ws_client.mjs"

llm:
  provider: "mock"
  model: "mock-received"
  base_url: "https://api.openai.com/v1"
  api_key: ""
  api_key_path: ""
  mock_response: "[openjarvis][DEBUG] 测试回复"
```

## Feishu 接入要求

当前推荐使用长连接模式，本地直接跑，不需要公网 webhook。

要真正让飞书消息进来并回消息，你需要准备这些外部条件：

- 一个飞书企业自建应用
- 已开启机器人能力
- 已订阅事件 `im.message.receive_v1`
- 在飞书后台把订阅方式切成 `使用长连接接收事件`

如果你仍然要用 webhook：

- 需要一个公网 IPv4 请求地址
- 如果开启了 `verification_token`，需要和 `config.yaml` 保持一致
- 当前这版运行时未实现 webhook 模式
- `Encrypt Key` 先留空

如果要真正回发消息到 Feishu，还需要：

- `feishu.app_id`
- `feishu.app_secret`
- 机器人在目标群内，或者用户在机器人可用范围内
- `feishu.dry_run: false`

## LLM 接入要求

当前默认是：

- `llm.provider: mock`
- 直接固定返回 `[openjarvis][DEBUG] 测试回复`

如果你要切到真实模型，需要提供：

- `llm.provider: openai_compatible` 或 `openai` 或 `deepseek`
- `llm.base_url`
- `llm.api_key` 或 `llm.api_key_path`
- `llm.model`
- agent 的默认 system prompt 当前写死在代码里，不走配置文件

## 当前限制

- 只处理文本消息
- 只发送文本消息
- 长连接入口当前通过官方 Node SDK sidecar 接入，再转给 Rust router
- 当前这版运行时未实现 webhook 模式
- 当前 ReAct loop 只支持一轮工具调用
- 工具已经注册，但还没有做权限审批和沙箱隔离
- 不支持 session/memory/thread 持久化
- 当前虽然走 `register_channels` 批量注册，但实际只内置了 `feishu` 一个 channel
