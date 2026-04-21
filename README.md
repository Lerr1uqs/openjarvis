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
- 已实现本地 memory feature
  - 工作区 `./.openjarvis/memory/{active,passive}` markdown 仓库
  - thread init system prompt注入 active memory `keyword -> path` catalog
  - `memory` toolset: `memory_get` / `memory_search` / `memory_write` / `memory_list`
- 已实现两类 LLM provider
  - `mock`
  - `openai_compatible` 及其别名 `openai` / `deepseek`
- 已实现 Feishu 文本消息回发
- 已实现 tool call / tool result 事件回发到当前群聊
- 已实现 bubblewrap sandbox，并在 proxy / command child 两级接入 namespace + Landlock + Seccomp 分层收口

## 本地启动

推荐先准备这些本地依赖：

- Rust toolchain
- Node.js / npm
- Linux 下如需跑 sandbox 相关能力或测试，安装 `bubblewrap`（`bwrap`）
- Linux 下如需启用严格 bubblewrap kernel enforcement，内核需启用 Landlock，且当前环境需要支持 seccomp filter
- Linux 下如需排查 sandbox / helper 进程启动问题，安装 `strace`
- Linux 下如需验证 Landlock ABI，可查看 `/sys/kernel/security/landlock` 或直接运行 sandbox 相关测试观察 fail-fast 错误

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

## Sandbox 调试

- `config/capabilities.yaml` 中的 `sandbox.bubblewrap.namespaces / baseline_seccomp_profile / proxy_landlock_profile / command_profiles / compatibility` 定义了 bubblewrap kernel enforcement plan。
- 当 `compatibility.require_landlock=true` 或 `compatibility.require_seccomp=true` 时，缺少对应内核能力会在 sandbox 启动阶段直接失败，而不是降级运行。
- `command_profiles.selected_profile` 控制 `exec_command` 默认使用的 child profile；`readonly` profile 会拒绝写入 workspace，可用于本地排查 child helper 是否生效。
- 排查 proxy 启动失败时，可结合 `RUST_LOG=debug` 与 `strace -f cargo test tests::agent::sandbox -- --nocapture` 观察 helper 收口位置。

## 本地 Skill

本地 skill 默认放在当前工作区的 `.openjarvis/skills/`。

安装首版 curated `acpx` skill：

```bash
openjarvis skill install acpx
```

卸载本地 `acpx` skill：

```bash
openjarvis skill uninstall acpx
```

安装完成后，skill 文件会落到：

```text
./.openjarvis/skills/acpx/SKILL.md
```

运行时需要让 agent 显式加载某个本地 skill 时，仍然通过现有 `load_skill` 工具或启动参数 `--load-skill <name>` 使用。

默认监听地址：

- 当前长连接模式下不需要 HTTP 入口
- 默认会自动拉起一个 Node sidecar，通过飞书官方 SDK 建立长连接
- 启动时会按配置批量注册全部 channel；当前内置实现只有 `feishu`
- `AppConfig` 在启动期完成加载和必要 override 后，会被安装成进程级只读快照
- 顶层 runtime/worker/provider 可以直接从全局只读配置装配；测试和嵌入场景仍保留显式 `from_config(...)` / `build_provider(...)` 入口

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
  context_window_tokens: 8192
  tokenizer: "chars_div4"

agent:
  compact:
    enabled: false
    auto_compact: false
    runtime_threshold_ratio: 0.85
    tool_visible_threshold_ratio: 0.70
    reserved_output_tokens: 1024
    # mock_compacted_assistant: "这是压缩后的上下文，请基于这些信息继续当前任务：..."
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
- `llm.context_window_tokens`
- `llm.tokenizer`，当前内置 `chars_div4`
- agent 的默认 system prompt 当前写死在代码里，不走配置文件

如果你要开启 runtime compact / auto-compact，还需要：

- `agent.compact.enabled: true`
- `agent.compact.runtime_threshold_ratio`
- `agent.compact.tool_visible_threshold_ratio`
- `agent.compact.reserved_output_tokens`
- `agent.compact.auto_compact: true` 时，模型会收到预算信息，并在软阈值后看到 `compact` 工具
- 如需本地调试 compact，可直接配置 `agent.compact.mock_compacted_assistant`，这样会走静态 mock，不再额外调用 compact LLM

## 当前限制

- 只处理文本消息
- 只发送文本消息
- 长连接入口当前通过官方 Node SDK sidecar 接入，再转给 Rust router
- 当前这版运行时未实现 webhook 模式
- 当前 ReAct loop 只支持一轮工具调用
- compact 当前只压缩线程 `chat`，不会压缩 `system` 和 thread init 注入的 active memory catalog
- 工具已经注册，但还没有做权限审批和沙箱隔离
- memory 当前只支持本地 markdown 持久化与词法检索，不支持 embedding / FTS / 热刷新当前线程 catalog
- 当前虽然走 `register_channels` 批量注册，但实际只内置了 `feishu` 一个 channel
