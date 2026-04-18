## Why

当前项目里，`ThreadAgentKind`、feature 初始化、tool visibility 和 toolset catalog 仍然分散在多处决定，导致“某个线程到底是什么 agent，因此应该拥有哪些 prompt、tool、feature”没有单一真相。随着 main thread、browser subagent 等类型继续扩展，这种分散判断会越来越难维护，也会让 child thread 继续依赖 scattered special-case。

## What Changes

- 为每个 `ThreadAgentKind` 定义统一的 capability profile，集中描述该 kind 的 `system prompt`、默认工具、允许使用的工具，以及允许使用的 features。
- 规定 `initialize_thread` 必须通过 `kind + capability profile` 完成线程初始化，而不是在初始化链路中零散拼装各类 prompt、tool 和 feature 特判。
- 规定 thread-scoped tool visibility 与 toolset catalog 也必须受 `ThreadAgentKind` profile 约束，而不只是初始化阶段生效。
- 规定 subagent 例如 `browser` 的能力边界由其 kind profile 决定；首版 `browser` 仅保留浏览器职责，不启用 `memory`、`skill`、`subagent` 等 feature。
- 规定 main thread 的 feature 解析继续允许配置驱动，但最终结果必须落在 `Main` kind 允许的能力范围内。
- 规定 `Main` thread 不直接暴露 `browser` 工作套件；需要浏览器能力时，只能通过 `subagent` 调度 `Browser` kind child thread。

## Capabilities

### New Capabilities
- `thread-agent-capability-profile`: 定义 `ThreadAgentKind` 如何拥有线程初始化与运行时能力边界的统一 profile 真相。

### Modified Capabilities
- `thread-context-runtime`: 变更线程初始化语义，使其通过 kind profile 一次性决定稳定 prompt、默认工具和 feature 注入边界。
- `thread-managed-toolsets`: 变更线程工具显隐与 toolset catalog 语义，使其受当前 thread agent kind 允许范围约束，而不是只受注册状态和线程已加载状态约束。

## Impact

- Affected code: `src/thread.rs`、`src/thread/agent.rs`、`src/agent/feature/**`、`src/agent/tool/**` 以及对应测试。
- Runtime impact: 不同 `ThreadAgentKind` 的线程将看到不同的 feature prompt、always-visible tools 和可加载 toolsets。
- Behavior impact: `browser` 这类 subagent 将不再默认继承 main thread 的 feature 能力，只保留其 kind profile 明确允许的能力。
- Behavior impact: main thread 不再直接加载或看到 `browser` toolset，浏览器能力入口统一收敛到 `subagent -> Browser`。
- Future impact: 后续若需要通过配置限制某类 subagent 可用 skill，可在 kind profile 边界之上继续扩展，而不需要再重构线程初始化真相。
