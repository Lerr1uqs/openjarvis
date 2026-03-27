## ADDED Requirements

### Requirement: Tool registry SHALL load MCP servers from tool configuration
The system SHALL load MCP server definitions from `agent.tool.mcp.servers` and manage them as part of the tool runtime rather than as a separate agent-level registry.

#### Scenario: Build runtime from configuration with MCP servers
- **WHEN** application configuration contains one or more MCP server definitions under `agent.tool.mcp.servers`
- **THEN** `ToolRegistry` SHALL create managed MCP server entries with their configured transport, enabled state, and connection settings

### Requirement: Tool registry SHALL only expose healthy enabled MCP tools
The system SHALL probe each enabled MCP server during runtime initialization by completing the MCP lifecycle handshake and listing tools. Only servers that pass probing SHALL contribute tools to the model-visible tool list.

#### Scenario: Healthy server exports tools
- **WHEN** an enabled MCP server completes initialization and returns tools successfully
- **THEN** the tool registry SHALL expose each remote tool with a namespaced name in the form `mcp__<server>__<tool>`

#### Scenario: Unhealthy server stays hidden
- **WHEN** an enabled MCP server fails initialization, tool listing, or transport connection
- **THEN** the tool registry SHALL record the server as unhealthy and SHALL NOT expose that server's tools in the model-visible tool list

### Requirement: Tool registry SHALL provide MCP runtime management APIs
The system SHALL provide runtime MCP management operations through the tool subsystem for other components to query servers, query remote tools, enable servers, disable servers, and refresh server state.

#### Scenario: Disable server removes tools
- **WHEN** a caller disables a managed MCP server through the runtime MCP management API
- **THEN** the tool registry SHALL stop exposing that server's tools to the model and SHALL report the server as disabled

#### Scenario: Enable server re-probes transport
- **WHEN** a caller enables a previously disabled MCP server through the runtime MCP management API
- **THEN** the tool registry SHALL reconnect and probe the server before exposing any of its tools

### Requirement: Tool registry SHALL route namespaced MCP tool calls to remote MCP servers
The system SHALL accept namespaced MCP tool calls through the same tool invocation entry point used for builtin tools and SHALL route those calls to the corresponding remote MCP server and tool.

#### Scenario: Agent loop calls MCP tool through normal tool path
- **WHEN** the model returns a tool call named `mcp__demo_stdio__echo`
- **THEN** the tool registry SHALL invoke the mapped MCP server tool and return the normalized tool result to the agent loop

### Requirement: Demo MCP servers SHALL be available for verification
The system SHALL provide demo-only internal MCP servers for local verification and automated tests, and those servers SHALL be clearly documented in code as non-production support utilities.

#### Scenario: Demo MCP server supports stdio verification
- **WHEN** automated tests spawn the internal demo stdio MCP server
- **THEN** the server SHALL respond to initialization, tool discovery, and at least one tool call successfully

#### Scenario: Demo MCP server supports streamable HTTP verification
- **WHEN** automated tests start the internal demo streamable HTTP MCP server
- **THEN** the server SHALL respond to initialization, tool discovery, and at least one tool call successfully
