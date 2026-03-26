## Why

The current agent runtime exposes only one shared tool registry shape and does not provide a thread-scoped way to progressively load and unload non-basic toolsets. This causes prompt-context inflation when too many tools are exposed up front, and it also makes it hard for the agent to manage tool availability as part of one long-running thread session.

## What Changes

- Add a thread-managed toolset runtime model so each internal thread maintains its own loaded toolset state instead of sharing one global non-basic tool visibility set.
- Introduce program-defined toolsets as first-class runtime units. A toolset can wrap builtin tool implementations, MCP-backed tools, or a mixed preset bundle that is already known by the program.
- Add always-visible agent tools `load_toolset` and `unload_toolset` so the model can decide when to attach or detach one toolset during a thread.
- Add a lightweight toolset catalog prompt that describes available toolsets without exposing every non-basic tool schema until the toolset is loaded.
- Persist thread-level loaded toolset state and tool call history so the runtime can reconstruct per-thread tool availability and preserve auditability.
- Update the agent loop so it refreshes the visible tool list before each model generation step, allowing toolset load/unload actions to take effect within the same ReAct turn.

## Capabilities

### New Capabilities
- `thread-managed-toolsets`: Manage program-defined toolsets per internal thread and expose agent-callable load/unload operations for progressive tool availability.

### Modified Capabilities

## Impact

- Affected code: `src/agent/agent_loop.rs`, `src/agent/runtime.rs`, `src/agent/tool/**`, `src/agent/worker.rs`, `src/router.rs`, `src/session.rs`, `src/thread.rs`, and mapped tests under `tests/agent/**`, `tests/session.rs`, and `tests/thread.rs`.
- Affected runtime behavior: non-basic tools are no longer all exposed by default; each thread runtime will decide visible tools from its loaded toolsets before every model call.
- API impact: `ToolRegistry` will shift toward a program-wide tool catalog plus thread-scoped runtime projection, and the agent tool surface will gain `load_toolset` and `unload_toolset`.
- Persistence impact: thread metadata must store loaded toolset state in addition to the existing normalized message history.
- Future extension note: load/unload hooks should leave room for later approval and policy checks, but approval is out of scope for this change.
