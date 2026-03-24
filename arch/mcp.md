mcp feature:
- 可以在配置文件中定义 json转换为yaml形式
- 支持stdio/sse/http三种形式 必须都测试到 编写testcase的mcpserver进行交互测试 确保功能都正常调用到
- 在程序启动的时候进行测试其中的功能（应该有check health的方式，不行就不要进入tools）并标记哪些mcp有问题 类似于 要提供查询接口后期可能要给外部查询
```
 Manage MCP servers
 1 server

 ❯ 1. jadx-mcp-server            ✔ connected · Enter to view details
```
- 支持enable disable接口 并且是能够实时的进行开关 也就是MCP让ToolRegisry管理 通过tools接口导出buildin tools+mcp tools 并且对tools能有enum标识进行区分

```json
{
  "mcpServers": {
    "tavily": {
      "command": "npx",
      "args": ["-y", "@mcptools/mcp-tavily"],
      "env": {
        "TAVILY_API_KEY": "your-api-key"
      }
    }
  }
}
```

如果是ut的话 可以考虑手写一个mcp server 在ut中让Agent去执行它 看能不能完成相应的效果