## Context

OpenJarvis already has a unified tool contract that the LLM consumes through `ToolRegistry`, but MCP is currently modeled as a separate placeholder registry under `agent`. That split prevents MCP tools from entering the same discovery and invocation path as builtin tools, and it also makes startup health checks and runtime management awkward because the real owner of tool exposure is `ToolRegistry`.

The requested direction is to treat MCP as a managed tool source. That means MCP should live under `src/agent/tool/mcp/`, be configured through the tool section, and be surfaced through runtime APIs that other components can call. The model should only see healthy, enabled remote tools, while management operations such as enable/disable/list remain outside the model tool list.

## Goals / Non-Goals

**Goals:**
- Make `ToolRegistry` the single owner of builtin tools and MCP-backed tools.
- Support `stdio` and `streamable_http` MCP transports.
- Perform MCP startup probing before exporting tools.
- Support runtime `enable`, `disable`, `refresh`, and query operations for MCP servers.
- Provide demo-only internal MCP servers that validate the protocol path in tests and local runs.
- Preserve the current agent loop contract so MCP tools look like normal tools to the model.

**Non-Goals:**
- Supporting legacy SSE transport.
- Exposing MCP management operations as agent-callable tools.
- Implementing MCP resources, prompts, sampling, elicitation, or task protocols in this change.
- Building an HTTP admin API in this change.

## Decisions

### 1. MCP moves under `src/agent/tool/mcp/`

`ToolRegistry` already owns tool listing and tool invocation, so MCP belongs there rather than as a peer runtime registry. This removes duplicated ownership and lets `AgentRuntime` expose MCP through `runtime.tools().mcp()`.

Alternative considered:
- Keep `src/agent/mcp.rs` and bridge it into `ToolRegistry`.
  Rejected because it keeps tool ownership split across modules and complicates startup/config wiring.

### 2. MCP tools are exported with namespaced stable names

Remote tool names are normalized into `mcp__<server>__<tool>`. The namespace prevents collisions with builtin tools and makes it obvious which server a tool belongs to.

Alternative considered:
- Reuse the remote tool name directly.
  Rejected because name collisions with builtin tools or multiple MCP servers would be unavoidable.

### 3. Only healthy and enabled MCP servers contribute tools

On startup, each configured enabled server is connected, initialized, and queried with `tools/list`. If the handshake succeeds, the server state becomes healthy and its tools are registered for model discovery. If the probe fails, the server remains tracked with an unhealthy status and last error, but its tools stay hidden from the model.

Alternative considered:
- Always expose configured tools and fail later during calls.
  Rejected because the model would see broken tools and waste turns on avoidable failures.

### 4. Management APIs live on the tool side, not in the model tool list

`ToolRegistry` will expose a dedicated MCP manager view for other program components to call, including listing servers, listing remote tools, enabling, disabling, and refreshing. These are runtime management methods and are intentionally not surfaced as LLM-callable tools.

Alternative considered:
- Add builtin tools such as `mcp_enable` and `mcp_disable`.
  Rejected because the user explicitly wants these APIs for components, not for the model.

### 5. Demo MCP servers are internal subcommands

Demo servers will be implemented as internal subcommands in the main binary so tests can spawn the same executable for real protocol coverage without introducing extra packaging friction. These servers will be clearly marked as demo-only in code comments and config-facing descriptions.

Alternative considered:
- Separate binaries under `src/bin/`.
  Rejected because the user prefers internal subcommands and the main binary is enough for local protocol verification.

## Risks / Trade-offs

- [New dependency complexity] -> Use one focused MCP crate and keep protocol scope to `tools` plus lifecycle handshake only.
- [Startup latency increases] -> Probe only enabled servers, perform minimal handshake plus `tools/list`, and keep failed servers tracked instead of retry-looping aggressively during boot.
- [Runtime state can become stale after remote changes] -> Provide explicit `refresh` APIs and refresh-tool registration paths.
- [Transport-specific behavior differs between `stdio` and HTTP] -> Use the same normalized server state model and cover both with real protocol tests.
- [Moving MCP under tools changes public module paths] -> Re-export the new public types from `agent::tool` and update internal imports in one refactor step.

## Migration Plan

1. Add the new tool-managed MCP configuration model and runtime types under `src/agent/tool/mcp/`.
2. Update `ToolRegistry` and `AgentRuntime` to own MCP through the tool module.
3. Migrate existing tests and public re-exports away from `src/agent/mcp.rs`.
4. Add demo internal MCP subcommands and transport-specific tests.
5. Remove the old standalone MCP placeholder module after the new path is green.

Rollback strategy:
- Revert the change set and fall back to builtin tools only. No persisted data migration is involved.

## Open Questions

- None for this scoped change. The user already confirmed transport scope, naming, management surface, and demo-server strategy.
