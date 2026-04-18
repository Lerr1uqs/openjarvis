## Context

`model/feature.md` 已经把 feature 的边界定义得很清楚：feature 是一组可开关能力；它在 thread 初始化时注入 system prompt，让 Agent 知道可以用什么能力、如何找到对应功能或资源。

当前 subagent 运行时虽然已经存在，但它的“被主线程认知”还主要依赖工具层：

- 主线程能不能看到 subagent，依赖工具是否注册
- 主线程知不知道何时使用 subagent，依赖模型自己从工具名字和描述猜
- 当前到底有几个 subagent profile 可用，没有统一的稳定 prompt 入口

这与项目当前对 `memory`、`skill` 这类 feature 的建模方式不一致。

## Goals / Non-Goals

**Goals**

- 让 subagent 成为正式的主线程 feature，而不是只有工具名字的隐式能力。
- 让主线程初始化时拿到稳定的 subagent prompt，明确知道“有多少 subagent、各自做什么、何时用”。
- 让 subagent 管理工具与该 feature 绑定，避免 feature prompt 和 capability 暴露脱节。
- 保持 child thread 只感知自己的 profile，不继承父线程的 subagent 管理说明。

**Non-Goals**

- 本次不重新设计 subagent runtime 本身。
- 本次不新增新的 subagent profile 协议或动态安装协议。
- 本次不要求把 subagent 使用策略做成复杂的调度器或 planner。
- 本次不修改 `model/*.md` 组件文档，只通过 OpenSpec 描述新的行为要求。

## Decisions

### 1. Subagent 是父线程 feature，不是 child thread feature

`subagent-feature` 的核心职责是帮助主线程理解并调用子代理，因此它只属于父线程。child thread 本身已经通过 `ThreadAgentKind` 拿到自己的 profile prompt，不需要再看到“有哪些 subagent 可调用”的父线程说明。

结果：

- 主线程启用 `subagent-feature` 时看到 subagent 指引 prompt
- child thread 不看到这段 prompt
- child thread 不暴露 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent`

### 2. Prompt 内容必须来自可用 subagent catalog，而不是硬编码在主 prompt

主线程需要知道“当前有多少 subagent 可用”，这意味着 prompt 内容不能简单写死在基础 `main.md` prompt 里。系统需要在初始化时，根据当前运行时可用的 subagent profile 构建一份 catalog，并把它物化为稳定的 `System` 前缀。

catalog 至少包含：

- `available_count`
- `subagent_key`
- `role_summary`
- `when_to_use`
- `when_not_to_use`

### 3. Subagent feature 同时拥有 prompt 与管理工具可见性

如果只有 prompt 没有 capability ownership，会出现“prompt 说可以用 subagent，但工具不可见”或者“工具可见但 prompt 没说明”的双重真相。因此要求 subagent 管理工具属于 `Feature::Subagent` 或等价 feature 开关拥有的能力。

结果：

- feature 开启时：主线程同时拥有说明 prompt 和管理工具
- feature 关闭时：主线程既没有该 prompt，也不应看到这组管理工具

### 4. `when_to_use` 必须显式说明边界，而不是只列职责名

用户特别要求 prompt 里说明“什么情况下使用”。因此 prompt 不能只列出 profile 名字和一句简介，还要显式说明适用条件。首版至少要求说明如下边界：

- 当任务明显属于某个专用 profile、且需要子线程隔离上下文时，优先使用 subagent
- 当任务可以在主线程直接完成，或只需要一次简单工具调用时，不要为了形式感额外起 subagent
- 当任务需要复用子线程已有上下文时，优先复用已存在的对应 profile subagent

### 5. Prompt 更新时机沿用线程初始化 / 重初始化语义

subagent feature prompt 属于稳定 system prompt，不是 request-time live message。因此它的更新时机应与其他稳定 feature prompt 一致：

- 新线程初始化时生成
- 线程重初始化时重建
- 不要求在每一轮 request 中实时刷新

这意味着运行时新增了新的 subagent profile 后，已初始化线程不会立即自动看到更新内容；需要通过新线程或重初始化生效。
