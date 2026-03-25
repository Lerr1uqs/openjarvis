# MCP 架构

## 目标

MCP 现在作为 `ToolRegistry` 管理的一类 tool source，而不是独立的 `agent::mcp` 子系统。
对模型来说，MCP tool 和 builtin tool 共用同一套发现与调用路径；对程序其他组件来说，MCP 的启停、刷新和查询通过 `ToolRegistry::mcp()` 暴露。

## 当前范围

- 只支持两种传输：
  - `stdio`
  - `streamable_http`
- 不支持 legacy `SSE`
- 当前只覆盖 MCP `tools` 能力，不覆盖 resources / prompts / sampling / elicitation / tasks
- MCP 管理接口不是 HTTP 管理 API，也不是给 agent 直接调用的 builtin tool

## 模块归属

- MCP 代码放在 `src/agent/tool/mcp/`
- `ToolRegistry` 是 builtin tools + MCP tools 的统一入口
- `AgentRuntime` 通过 `runtime.tools()` 持有 `ToolRegistry`
- 其他组件通过 `runtime.tools().mcp()` 调用 MCP runtime API

## 配置模型

配置入口：

```yaml
agent:
  tool:
    mcp:
      servers:
        demo_stdio:
          enabled: true
          transport: stdio
          command: openjarvis
          args: ["internal-mcp", "demo-stdio"]

        demo_http:
          enabled: false
          transport: streamable_http
          url: "http://127.0.0.1:39090/mcp"
```

也支持独立 sidecar 文件：

```json
{
  "mcpServers": {
    "demo_stdio_file": {
      "command": "openjarvis",
      "args": ["internal-mcp", "demo-stdio"]
    },
    "demo_http_file": {
      "transport": "http",
      "url": "http://127.0.0.1:39090/mcp"
    }
  }
}
```

说明：

- `enabled=true` 的 server 会在启动时被探测
- `stdio` 需要 `command/args/env`
- `streamable_http` 需要 `url`
- `transport: http` 作为 `streamable_http` 的兼容别名仍可解析，但内部统一归一为 `streamable_http`
- sidecar 默认路径为 `./config/openjarvis/mcp.json`
- sidecar 相对 `AppConfig::from_path(...)` 指向的 YAML 根目录解析；即使 YAML 缺失，也允许只靠这个 JSON 加载 MCP
- sidecar 与 YAML 的 server 名称如果冲突，直接报错，不做静默覆盖

## 运行时状态

每个 MCP server 都有统一状态：

- `disabled`
- `healthy`
- `unhealthy`

运行时会保存：

- server 名称
- transport
- endpoint 摘要
- 是否启用
- 当前状态
- 当前导出的 tool 数量
- 最近一次探测时间
- 最近一次错误

## 启动与探测

启动时流程：

1. `ToolRegistry::from_config(...)` 读取 `agent.tool.mcp.servers`
2. 为每个 server 建立托管定义
3. 对 `enabled=true` 的 server 执行连接与初始化
4. 完成 MCP lifecycle handshake
5. 调用 `tools/list`
6. 只有探测成功的 server 才把 tools 挂入 registry

失败策略：

- server 仍然保留在托管列表中
- 状态标记为 `unhealthy`
- 不向模型暴露任何该 server 的 tools

## Tool 暴露方式

远端 tool 会被转换成稳定命名：

```text
mcp__<server>__<tool>
```

例如：

```text
mcp__demo_stdio__echo
mcp__demo_http__health_probe
```

这样可以：

- 避免与 builtin tool 冲突
- 避免不同 MCP server 之间的同名冲突
- 让调用链路清楚知道 tool 来源

每个 `ToolDefinition` 还会附带 `source` 元信息：

- `Builtin`
- `Mcp { server_name, remote_tool_name, transport }`

## 调用路径

模型返回 namespaced tool call 后，调用路径与 builtin tool 一致：

1. `AgentLoop` 收到 tool call
2. `ToolRegistry::call(...)` 查找 tool handler
3. 若为 MCP tool，则进入 `McpToolHandler`
4. `McpManager` 根据 `server_name + remote_tool_name` 转发到远端 MCP server
5. 返回值被归一成 OpenJarvis 的 `ToolCallResult`

归一后的结果包含：

- 文本化的 `content`
- `metadata`
- `is_error`

其中 `metadata` 会包含：

- `source: "mcp"`
- `server_name`
- `remote_tool_name`
- `structured_content`

## 管理 API

当前给其他组件暴露的运行时接口：

- `list_servers()`
- `list_tools()`
- `enable_server(name)`
- `disable_server(name)`
- `refresh_server(name)`

这些接口位于：

```rust
runtime.tools().mcp()
```

注意：

- 它们不是 agent 可调用的 builtin tool
- 它们也不是当前版本的 HTTP 管理接口

## Demo MCP

当前内置了 demo-only MCP server，用于联调和测试，后续可以逐步移除。

内部子命令：

```text
openjarvis internal-mcp demo-stdio
openjarvis internal-mcp demo-http --bind 127.0.0.1:39090
```

内置 demo tools：

- `echo`
- `sum`
- `health_probe`

约束：

- 这些 server 只用于验证协议能力
- 注释中必须明确标注为 demo-only，不能当成生产能力依赖

## 测试要求

当前实现已经覆盖：

- `agent.tool.mcp.servers` 配置加载与校验
- `config/openjarvis/mcp.json` sidecar 加载、合并与错误校验
- `stdio` demo server 真协议测试
- `streamable_http` demo server 真协议测试
- sidecar `mcp.json` 驱动的 `stdio` / `streamable_http` 真协议测试
- server 启停、刷新、异常状态测试
- namespaced MCP tool 在 `AgentLoop` / 外部 channel 消息链路中的完整执行测试
