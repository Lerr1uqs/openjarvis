## Why

当前 `subagent` 已经有运行时模型和管理工具，但主线程对这项能力的认知仍然主要依赖工具 schema 本身。按照  `model/feature.md` 的定义，feature 的职责应该是“在线程初始化时注入稳定 system prompt，让 Agent 知道有哪些能力、什么时候该用”。如果 `subagent` 继续只停留在工具层，主线程模型很难稳定理解：

- 当前到底有多少种 subagent 可用
- 每种 subagent 分别适合处理什么任务
- 什么情况下应该优先直接用当前线程工具，而不是派生 subagent

这会导致主线程对 subagent 的使用高度依赖即时推断，不符合当前项目已经为 `memory`、`skill` 这类 feature 建立的“稳定前缀注入 + feature-owned capability”模式。

## What Changes

- 新增 `subagent-feature` 能力，把 subagent 从“只有工具”提升为“主线程可启用的正式 feature”。
- 要求主线程在初始化或重初始化时注入稳定的 subagent feature prompt，至少说明：
  - 当前可用 subagent 的数量
  - 每个 subagent 的 `subagent_key`
  - 每个 subagent 的职责摘要
  - 什么时候应该使用该 subagent
  - 什么情况下不应该使用 subagent，而应直接在主线程完成
- 规定 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 这组主线程管理工具属于 `Feature::Subagent` 或等价 feature 开关拥有的能力，而不是永远裸露在所有主线程前。
- 规定 child thread 不继承父线程的 subagent feature prompt；subagent 自己只保留自己的 profile prompt 与工具真相。
- 规定 subagent feature prompt 必须基于“当前实际可用 subagent catalog”构建，而不是把 profile 列表永久硬编码在基础主线程 prompt 里。

## Capabilities

### New Capabilities

- `subagent-feature`: 定义 subagent 作为正式 thread feature 的 prompt 注入、能力暴露和使用指引。

### Modified Capabilities

- `thread-context-runtime`: 扩展线程初始化流程，使主线程在启用 subagent feature 时注入稳定的 subagent system prompt，并在 child thread 上显式排除这组父线程管理说明。

## Impact

- Affected code: `src/thread.rs`、`src/thread/agent.rs`、`src/agent/feature/**`、`src/agent/runtime.rs`、`src/agent/tool/**` 以及对应测试。
- Runtime impact: 主线程 tool visibility 将不再只由“是否是 main thread”决定，还要受 subagent feature 开关控制。
- Prompt impact: 主线程初始化 `System` 前缀会新增 subagent feature prompt；child thread 不会看到这部分 prompt。
- Behavior impact: 新增或减少可用 subagent profile 时，后续新建线程或重初始化线程看到的 subagent prompt 内容会随 catalog 更新。
