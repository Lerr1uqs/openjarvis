## Context

当前 `ThreadAgentKind` 已经负责选择 bundled system prompt，并为部分 kind 绑定默认 toolset；但 feature 解析、feature prompt 注入、always-visible tool 显隐、toolset catalog 暴露仍然散在 `ThreadRuntime`、`Thread` 和 feature init 模块中分别决定。这导致：

- `kind` 只能部分表达“这个线程是什么 agent”
- child thread 仍要依赖特判去裁掉不该继承的能力
- tool visibility 与初始化真相分离，容易出现“初始化没给，但后续又能看到”的能力穿透

这次设计希望把线程能力真相收口到 `ThreadAgentKind`，但不引入新的 `AgentRegister` 或其他额外概念，直接在现有 `ThreadAgentKind` 上建立 capability profile。

## Goals / Non-Goals

**Goals:**
- 让 `ThreadAgentKind` 成为线程初始化真相，统一决定稳定 prompt、默认工具和可用 feature。
- 让 thread-scoped tool visibility 与 toolset catalog 也受 kind profile 约束，而不是只在初始化时生效。
- 让 main thread 继续保留配置驱动 feature 的能力，但最终结果必须受 `Main` kind 允许范围约束。
- 让 `browser` 这类 subagent 在首版只保留其专用职责，不暴露 `memory`、`skill`、`subagent` 等 feature。

**Non-Goals:**
- 本次不引入新的 agent registry 或动态插件式 agent profile 注册协议。
- 本次不开放运行中切换 thread kind，也不支持线程初始化后再变更 capability profile。
- 本次不实现“通过配置限制某类 subagent 允许使用哪些 skill”；该能力留待后续在 kind profile 之上扩展。
- 本次不修改 `model/*.md` 组件文档。

## Decisions

### 1. 用 `ThreadAgentKind` 直接解析 capability profile，不新增 `AgentRegister`

`ThreadAgentKind` 已经是当前线程角色选择入口，因此继续让它扩展为 profile owner 最直接。profile 至少包含：

- `system_prompt`
- `default_bound_toolsets`
- `allowed_toolsets`
- `default_features`
- `allowed_features`

这样可以避免再引入一个并行的 registry 概念，减少线程能力真相分叉。

### 2. `kind` 负责能力边界真相，而不只是初始化默认值

如果 `kind` 只负责初始化默认值，后续工具显隐和 toolset 加载仍可能绕过初始化结果，导致能力穿透。因此本次定义：

- 初始化阶段通过 kind profile 生成线程初始 prompt / tool / feature
- 运行时阶段 visible tools、toolset catalog、toolset load/unload 也必须经过 kind profile 过滤

也就是说，`kind` 是能力边界真相，不只是 bootstrap 建议。

### 3. main thread 保留“配置驱动 + profile 限幅”，subagent 走 profile 固定边界

`Main` thread 仍允许通过现有 feature resolver 得到配置驱动结果，但最终需要与 `Main` kind 的 `allowed_features` 求交集。  
subagent 例如 `Browser` 则以 profile 明确给出固定边界，首版不启用 `memory`、`skill`、`subagent`。

这让 main thread 继续保持灵活，而 subagent 保持窄职责。

同时，`Main` thread 不直接拥有 `browser` 工作套件。需要浏览器能力时，主线程只能通过 `subagent` feature 调度 `Browser` kind child thread，而不是自己加载 `browser` toolset 后直接执行浏览器动作。

### 4. 默认绑定工具与可选 toolset 必须区分

像 `browser` 这样的默认能力属于 kind 真相的一部分，不应被当作“线程可随意装卸的可选 toolset”。因此需要区分：

- `default_bound_toolsets`: kind 固有能力，初始化时直接生效
- `allowed_toolsets`: 当前 kind 允许线程后续显式加载的可选 toolset

这样 `Browser` 可以默认拥有浏览器工具，但不因此获得 `load_skill` 或其他可选能力入口。
同时 `Main` 可以继续拥有其他可选 toolset 的入口，但 `browser` 不属于 `Main` 的可选 toolset 范围。

### 5. toolset catalog prompt 与 `load_toolset` / `unload_toolset` 也要按 kind 过滤

如果 thread 看不到某 toolset，但 catalog prompt 仍把它列出来，或者仍能看到 `load_toolset` 试图加载不允许的 toolset，那么线程真相仍然不一致。因此需要：

- catalog 只列当前 kind 允许的可选 toolsets
- 如果当前 kind 没有任何可选 toolset，则不暴露 `load_toolset` / `unload_toolset`
- 若尝试加载不在 kind 允许范围内的 toolset，系统必须拒绝
- `Main` thread 的 catalog 与 `load_toolset` 入口都不能把 `browser` 当作直接可用工作套件暴露给模型

## Risks / Trade-offs

- [Risk] `ThreadAgentKind` 承担更多职责，枚举定义会比现在更重。  
  → Mitigation: 把具体 profile 数据收口到独立 helper / struct，枚举本身只做解析入口。

- [Risk] 现有测试大多默认 main thread 具备更多 feature，调整后容易出现回归。  
  → Mitigation: 补齐主线程 / browser thread 的初始化、visible tools、toolset catalog 回归测试。

- [Risk] “默认绑定工具”与“可选 toolset”语义调整会影响当前 `thread-managed-toolsets` 的行为认知。  
  → Mitigation: 在 spec 里明确区分两者，并补充场景约束 catalog 与 load/unload 的新行为。

- [Risk] 未来若要开放 subagent skill 白名单，当前 profile 结构可能需要继续扩展。  
  → Mitigation: 当前先保留 `allowed_toolsets` / `allowed_features` 两层边界，为后续配置化限制留出入口。

## Migration Plan

- 先新增 capability profile 解析结构，并让 `ThreadAgentKind` 统一返回 profile。
- 再把 `initialize_thread`、feature resolver 结果过滤、tool visibility、toolset catalog/loading 全部切到 profile。
- 最后补齐 main/browser 的初始化与工具边界测试，确认现有 subagent/runtime change 不被破坏。

这次改动属于线程初始化与工具投影内部语义收口，不涉及外部 API 迁移。

## Open Questions

- `Main` 是否继续允许 `auto_compact`，还是也要通过 profile 显式区分默认启用与允许启用？
- 默认绑定 toolset 是否需要单独持久化为“不可卸载”类别，还是只要在运行时禁止其通过 `unload_toolset` 被移除即可？
- 后续若引入新的 subagent kind，是否需要为 profile 增加 `allowed_skills` 这类更细粒度边界字段？
