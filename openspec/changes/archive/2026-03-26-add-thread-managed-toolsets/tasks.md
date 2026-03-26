## 1. Thread Runtime and Toolset Catalog

- [x] 1.1 Introduce program-defined toolset definitions and catalog loading so non-basic capabilities are registered as named toolsets instead of being exposed eagerly
- [x] 1.2 Add a thread-scoped runtime manager keyed by internal `thread_id` to track loaded toolsets and live tool/runtime resources per thread
- [x] 1.3 Add always-visible `load_toolset` and `unload_toolset` tools that mutate only the current thread runtime and leave an approval/policy extension point for later

## 2. Agent Loop and Persistence

- [x] 2.1 Update the agent loop to rebuild the visible tool list from the current thread runtime before each model generation step
- [x] 2.2 Extend session/thread persistence to store loaded toolset names alongside normalized message history
- [x] 2.3 Add structured tool call event persistence for tool load, unload, and execution records so thread runtime state can be audited and reconstructed
- [x] 2.4 Rehydrate thread runtime state from persisted thread metadata before handling a new request for that internal thread

## 3. Toolset Integration

- [x] 3.1 Register at least one concrete non-basic toolset through the new toolset path and ensure its tool names remain stable and collision-safe
- [x] 3.2 Adapt MCP-backed tool exposure so MCP tools can participate in program-defined toolsets without leaking globally visible non-basic tools into every thread

## 4. Verification

- [x] 4.1 Add unit tests for toolset catalog exposure and `load_toolset`/`unload_toolset` behavior under `tests/agent/tool/`
- [x] 4.2 Add agent loop tests proving same-turn tool refresh after `load_toolset` and post-unload tool disappearance
- [x] 4.3 Add session/thread tests covering loaded toolset persistence, tool event persistence, and runtime reconstruction by internal thread
- [x] 4.4 Add isolation tests proving one thread's loaded toolsets do not affect another thread for the same user or channel tuple family
