## 1. Tool-Managed MCP Structure

- [x] 1.1 Move MCP runtime types under `src/agent/tool/mcp/` and update public re-exports away from the standalone agent-level MCP module
- [x] 1.2 Extend configuration and runtime wiring so `AgentRuntime` loads MCP through `ToolRegistry` from `agent.tool.mcp.servers`
- [x] 1.3 Extend tool definitions and registry behavior so builtin tools and healthy MCP tools share one listing and one call path while preserving source metadata

## 2. MCP Runtime and Demo Servers

- [x] 2.1 Implement managed MCP server state, startup probing, and runtime management APIs for list, enable, disable, and refresh
- [x] 2.2 Implement namespaced MCP tool mapping and remote tool call dispatch for `stdio` and `streamable_http`
- [x] 2.3 Add demo-only internal MCP servers as internal subcommands for real protocol verification in tests

## 3. Verification

- [x] 3.1 Add unit tests for MCP configuration loading and runtime management behavior under `tests/agent/tool/`
- [x] 3.2 Add protocol tests that exercise demo MCP servers over `stdio` and `streamable_http`
- [x] 3.3 Add an agent loop test that verifies a namespaced MCP tool call executes through the normal tool path
