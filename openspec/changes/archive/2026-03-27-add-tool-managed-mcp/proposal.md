## Why

The current MCP support is only a placeholder registry under `agent` and does not let the model discover or call remote MCP tools. This change is needed now to make MCP a real part of the tool runtime, with transport support, health checks, and management hooks that other components can use.

## What Changes

- Move MCP ownership under the `agent/tool` module so `ToolRegistry` becomes the single runtime entry point for builtin tools and MCP-backed tools.
- Add MCP configuration loading under `agent.tool.mcp.servers`, with support for `stdio` and `streamable_http` transports.
- Add MCP server connection, initialization, tool discovery, health tracking, and runtime enable/disable/refresh management.
- Expose healthy MCP tools through the same tool listing and call path used by builtin tools, with namespaced tool names in the form `mcp__<server>__<tool>`.
- Add demo-only internal MCP servers for local verification and automated tests.
- Remove the old standalone agent-level MCP placeholder model in favor of tool-managed MCP runtime state.

## Capabilities

### New Capabilities
- `tool-managed-mcp`: Load, manage, health-check, and expose MCP servers and tools through `ToolRegistry`.

### Modified Capabilities

## Impact

- Affected code: `src/agent/runtime.rs`, `src/agent/mod.rs`, `src/agent/tool/**`, `src/config.rs`, `src/main.rs`, `src/agent/agent_loop.rs`, and mapped tests under `tests/agent/**`.
- Affected runtime behavior: startup now probes configured MCP servers before exporting their tools.
- New dependency impact: add an MCP client/server Rust dependency for `stdio` and `streamable_http` transport support.
- API impact: `ToolRegistry` gains MCP management APIs for other program components; these are runtime APIs, not model-callable tools.
