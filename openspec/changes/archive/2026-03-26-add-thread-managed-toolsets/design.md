## Context

OpenJarvis already has a unified tool contract, but the runtime still assumes one shared tool registry view for model requests. That works for the basic four-tool set and for always-on tool sources, but it does not fit the new requirement to progressively load and unload non-basic toolsets inside one long-running internal thread.

The requested model is stricter than the current local-skill prompt pattern. Skills only inject instructions into prompt history, while toolsets must change the actual model-visible tool list and may also own runtime resources such as MCP client connections or browser sessions. The user also clarified that internal thread identity is already derived from `channel:user:external_thread`, so the correct isolation boundary is the internal thread, not the user.

## Goals / Non-Goals

**Goals:**
- Keep basic tools always available while making non-basic tools visible only when their toolset is loaded.
- Make toolset load state isolated per internal thread runtime.
- Let the agent explicitly call `load_toolset` and `unload_toolset`.
- Allow one toolset to wrap builtin tools, MCP-backed tools, or a mixed program-defined preset bundle.
- Refresh the tool list before every model generation step so load/unload operations take effect within the same ReAct turn.
- Persist enough thread metadata to reconstruct loaded toolsets and tool call history after reload.

**Non-Goals:**
- Supporting arbitrary user-defined toolset composition in this change.
- Auto-unloading toolsets at the end of every turn.
- Adding approval gates to `load_toolset` or `unload_toolset` in this change.
- Changing channel-level thread resolution semantics.
- Replacing the existing MCP transport model or adding new MCP protocols.

## Decisions

### 1. Split global tool knowledge from thread runtime state

The current `ToolRegistry` shape is close to a global registry, but progressive toolsets require per-thread visibility and lifecycle state. The runtime will therefore separate:

- a program-wide tool catalog that knows all builtin tools, toolsets, and MCP-backed definitions
- a thread runtime that tracks which toolsets are currently loaded for one internal thread and which live handlers/resources are attached

This avoids cross-thread leakage while keeping tool discovery deterministic.

Alternative considered:
- Mutate one shared registry in place for every load/unload.
  Rejected because one thread could accidentally change the visible tool surface for another thread.

### 2. Thread runtime is keyed by internal thread identity

Each internal thread gets its own runtime view keyed by the existing internal `thread_id` resolution. This runtime owns loaded toolset state and any live resources created by those toolsets.

Alternative considered:
- Keep one runtime per user.
  Rejected because one user can naturally have multiple concurrent internal threads, and tool availability must remain isolated across them.

### 3. Toolsets are program-defined manifests, not ad hoc tool filters

A toolset is a first-class preset implemented by the program and described by a manifest, such as `toolsets/browser/`. The manifest defines:

- toolset name and short description
- the tool handlers or delegated tool sources it exposes
- optional load and unload lifecycle hooks

This keeps the surface predictable and lets MCP-backed toolsets remain curated rather than dynamically exposing arbitrary remote subsets.

Alternative considered:
- Require every toolset to be configured from external `include_tools` lists.
  Rejected because the user expects toolsets to be built into the program as known presets, with allowlists as an internal implementation detail rather than an external contract.

### 4. `load_toolset` and `unload_toolset` are always-visible builtin tools

The model needs a stable way to change the current thread's visible toolsets. Two builtin tools will always remain visible:

- `load_toolset`
- `unload_toolset`

They operate only on the current thread runtime. Their result payloads stay short and operational, while the human-readable catalog of available toolsets is injected as a compact system prompt.

Alternative considered:
- Auto-load toolsets purely from prompt instructions without a dedicated tool call.
  Rejected because the runtime would have no explicit event to attach handlers, persist state, or emit audit records.

### 5. Agent loop refreshes visible tools before every generation

The current loop snapshots tools once per run, which prevents same-turn toolset changes from taking effect. The loop will instead rebuild the visible tool list from the current thread runtime before every `generate()` call. This preserves the ReAct contract while enabling:

- load a toolset
- continue the same turn
- call newly visible tools immediately

Alternative considered:
- Only refresh tools on the next incoming user turn.
  Rejected because it wastes a turn and makes toolset loading much less useful.

### 6. Thread persistence stores both normalized messages and toolset state

Persisted history alone is not enough to recover which toolsets should still be loaded. Thread state must therefore include:

- normalized message history
- loaded toolset names
- structured tool call records for audit and future policy hooks

On runtime restart or rehydration, the thread runtime can rebuild its live handlers from the stored loaded toolset set.

Alternative considered:
- Infer loaded toolsets only by replaying tool call history.
  Rejected because inference is brittle and cannot distinguish “previously loaded but not yet unloaded” from “used once in the past”.

### 7. Tool names remain namespaced by toolset when needed

Tool names exposed from non-basic toolsets should remain stable and collision-safe. Toolsets may expose tools under namespaced names such as `browser__open_page` or map curated MCP tools into stable toolset-owned names.

Alternative considered:
- Reuse remote or local raw tool names directly.
  Rejected because name collisions across multiple loaded toolsets become likely and would make routing ambiguous.

## Risks / Trade-offs

- [Thread runtime lifetime management becomes more complex] -> Keep persisted state declarative and rebuild live resources from toolset manifests instead of persisting runtime objects directly.
- [Agent may forget to unload an expensive toolset] -> Support explicit `unload_toolset` now and leave room for future idle eviction or policy cleanup.
- [Refreshing tool schemas before each generation adds overhead] -> Limit the refresh to visible tool projection from thread state, not full rediscovery of every program-defined toolset.
- [MCP-backed toolsets can still introduce transport latency] -> Keep MCP server management under the shared tool infrastructure while letting thread runtimes decide whether those tools are currently exposed.
- [Persisting structured tool events increases storage volume] -> Store concise event records and keep large tool outputs in normalized message history only when they are already part of the conversation contract.

## Migration Plan

1. Introduce program-defined toolset manifests and a thread runtime manager without changing existing basic tool behavior.
2. Add `load_toolset` and `unload_toolset`, plus compact toolset catalog prompt generation.
3. Update the agent loop to refresh visible tools before each model generation step.
4. Extend session/thread persistence to store loaded toolsets and structured tool events.
5. Migrate MCP-backed and other non-basic tool bundles to register through toolset manifests.
6. Add runtime reconstruction and isolation tests, then remove any legacy assumptions that one shared tool list is sufficient.

Rollback strategy:
- Revert the change and fall back to the current shared tool registry behavior with basic tools only. Persisted `loaded_toolsets` metadata can be ignored safely because it is additive.

## Open Questions

- None for this scoped change. Approval and policy checks are intentionally deferred but the load/unload path will keep an extension point for them.
