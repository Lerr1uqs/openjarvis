## Context

当前代码已经把稳定 prompt 注入收口到线程初始化路径，但具体组织方式仍偏向运行时行为抽象：

- `src/agent/feature/mod.rs` 通过 `FeaturePromptProvider` trait 和固定 provider 列表生成 feature prompts；
- feature 是否启用、启用后要注入哪些 usage prompt、是否附带工具集，没有一个面向线程初始化的闭集模型；
- channel/user 维度的 feature 配置还不能直接映射成“这个线程最终启用了哪些 feature”；
- 模型调用 shell/命令类工具时仍缺少宿主环境事实，尤其是 OS 与 shell 差异。

这个 change 的本质不是再引入一个新的 provider 层，而是承认当前 feature 是闭集能力集合，并把“配置解析 -> 线程初始化注入 -> 工具启用”拉成一条显式链路。

## Goals / Non-Goals

**Goals:**

- 用闭集 `Feature` 枚举表达线程初始化可启用的能力集合。
- 让 channel/user 配置可以解析成稳定的 `Features`，并在初始化时统一生效。
- 让线程把最终生效的 `Features` 持久化为自己的正式状态，而不是每次恢复都重新依赖外部配置。
- 让每个 feature 显式声明自己的稳定 `System` prompt、工具启用和必要状态初始化。
- 为线程增加稳定的运行时环境感知提示，覆盖当前 OS 与 shell 基本事实。
- 保持首轮请求即可看到正确的 feature prompts 和 feature-owned tools。

**Non-Goals:**

- 不引入新的外部 feature 插件机制、provider registry 或 trait object 扩展点。
- 不重做现有 tool routing、tool schema 或 command runtime 协议。
- 不在这个 change 中细化最终配置文件 schema 的所有字段命名，只先固化解析边界和默认行为。
- 不把运行时环境感知做成每轮动态刷新消息；首版只解决初始化注入。

## Decisions

### 1. 用闭集 `Feature` 枚举替代统一 provider trait

系统将引入闭集 `Feature` 枚举，例如：

- `Memory`
- `Skill`
- `AutoCompact`

并用 `Features` 容器表达一个线程最终启用的 feature 集合。每个枚举值通过显式分支完成三类物化：

- 追加稳定 usage/system prompt
- 预启用该 feature 拥有的工具集或工具可见性输入
- 写入必要的线程 feature state

这意味着实现上可以使用 `impl Feature` 或专用 helper 函数，但不会再以 `FeaturePromptProvider` trait 作为主模型。

拒绝方案：

- 继续沿用 `FeaturePromptProvider` trait，并为每个 feature 单独实现 provider  
  原因：当前 feature 集合是闭集，trait 只会把可枚举的初始化规则变成额外抽象层，增加调试和配置映射复杂度。

### 2. 在初始化前先解析 `channel + user -> Features`

系统会在创建新线程、准备执行初始化前，先通过显式解析入口得到目标线程的默认 `Features`。这些解析结果会立即写入线程持久化状态，成为该线程后续恢复与运行时判断使用的 feature 真相。首版解析规则为：

- 若 channel/user 维度存在显式 feature 配置，则按该配置返回
- 当前开发阶段若没有显式配置，默认返回 `Features::all()`

解析结果必须是稳定有序集合，不依赖原始配置的声明顺序。这样线程初始化、测试断言和日志都能得到一致顺序。线程恢复后，系统直接读取持久化的 `enabled_features`，而不是再次向 `channel/user` 配置求值。

拒绝方案：

- 在各 feature 注入点内部各自读取配置，自行决定是否生效  
  原因：会重新形成“feature 判断分散在多处”的问题，也不利于审计某个线程到底启用了哪些能力。

### 3. 持久化的 `enabled_features` 是线程真相，`auto_compact_override` 并入 `Feature::AutoCompact`

系统会把线程最终启用的 `Features` 作为正式 thread state 持久化，例如 `enabled_features: BTreeSet<Feature>`。其中 `Feature::AutoCompact` 表示该线程启用了 auto-compact 语义，因此当前独立的 `auto_compact_override` 字段会被收编并删除，不再保留一条单独的 override 路径。

对应语义变为：

- 新线程创建时，由 resolver 给出默认 `enabled_features`
- 线程恢复时，以已持久化的 `enabled_features` 为准
- 若后续需要修改某个线程是否启用 auto-compact，本质上是增删 `Feature::AutoCompact`

这样可以保证：

- feature 集合与线程初始化写入的稳定 `System` 前缀来源一致
- `AutoCompact` 不再是特殊字段，而是普通 feature
- 线程重启前后不会因为外部配置变化而偷偷改变 auto-compact 语义

拒绝方案：

- 保留 `auto_compact_override: Option<bool>`，同时再加 `enabled_features`  
  原因：同一语义会出现两套事实来源，后续判定到底看 override 还是看 feature 集合会变得混乱。

### 4. 线程初始化顺序固定为“基础角色 -> 环境感知 -> feature 物化”

线程初始化将采用固定顺序：

1. 将基础角色/系统 prompt 写入 `Thread.messages()` 开头前缀
2. 将运行时环境感知 prompt 追加到该稳定 `System` 前缀
3. 按稳定顺序遍历 `Features`，依次物化各 feature 的 prompt、工具和状态

参考伪代码如下：

```python
def init_thread(&mut thread, features: Features):
    thread.push(predefined_role)
    thread.push(build_shell_env_perception())  # 当前是 Windows 还是 Linux，shell 是什么

    for feat in features:
        thread.enable(feat)

    if thread.is_enabled(Feature::Memory):
        thread.push(MemoryRepo.usage())
        thread.enable_tools(MemoryRepo.tools())

    if thread.is_enabled(Feature::Skill):
        thread.push(SkillRegistry.usage())
        thread.enable_tools(SkillRegistry.tools())

    if thread.is_enabled(Feature::AutoCompact):
        thread.push(AutoCompact.usage())
        thread.push(compact_tools())
```

这里要点是：

- 先写稳定 `System` 前缀，再启用 feature
- feature 是否启用以 `thread.enable(...)` 后的线程状态为准
- `Memory`、`Skill` 这类 feature 同时负责 usage prompt 和工具启用
- `AutoCompact` 这类 feature 会同时注入 usage prompt、compact 工具可见性输入和状态输入，不必强行映射成独立 toolset

其中环境感知不作为普通业务 feature，而是独立的初始化阶段能力。原因是它表达的是宿主环境事实，不应被 channel/user feature 开关随意关闭，也不适合和 `Memory`、`Skill` 这类业务 feature 混在同一枚举责任里。

拒绝方案：

- 把环境感知也塞进 `Feature` 枚举，例如 `Feature::ShellEnv`  
  原因：环境感知不是业务 feature 开关，而是线程执行环境的基础事实，更适合作为固定初始化步骤。

### 5. feature-owned tools 复用现有线程工具状态，而不是额外开一套 feature tools 容器

feature 若附带工具能力，系统不会再单独维护一份“feature tools”容器，而是直接复用线程已有的工具状态模型：

- 对可加载 toolset，初始化时把对应 toolset 预加载进线程工具状态
- 对条件可见 builtin tool，例如 `compact`，仍通过线程 feature state + 可见性投影决定是否暴露

例如：

- `Feature::Memory` 可以在初始化时预加载 `memory` toolset
- `Feature::AutoCompact` 不直接加载独立 toolset，但会打开 auto-compact 的线程状态输入，并注入 `compact_tools()` 对应的工具可见性

这样可以保持工具显隐入口仍然只有线程工具状态和 visible-tools projection，避免 feature 和 toolset 两套机制并存。

拒绝方案：

- 为 feature 另外维护一组“直接暴露给模型的工具列表”  
  原因：这会绕过现有线程工具可见性与持久化审计边界，后续难以统一处理加载、恢复和日志。

### 6. 运行时环境感知通过专门构造器生成稳定快照，未知值显式暴露为 unknown

系统将新增专门的运行时环境感知构造器，例如 `RuntimeEnvironmentPerception`。它负责采集并格式化最小必要环境事实，例如：

- OS family/平台类型
- 默认 shell 或命令执行 shell
- 基本路径/命令约定提示

构造结果在初始化时生成一次，并作为稳定 `System` message 写入线程前缀。若某个字段无法可靠探测，系统必须显式写成 `unknown` 或等价占位，而不是猜测一个 shell/平台。

拒绝方案：

- 不注入环境事实，只让模型自行猜测  
  原因：这会持续造成错误命令、错误路径风格和错误 shell 语法。
- 在每次工具调用前临时拼一段环境提示  
  原因：环境事实属于线程稳定前缀，重复拼接会增加噪声，也不利于审计。

## Risks / Trade-offs

- [Risk] 新增 feature 需要修改枚举和初始化分支，扩展成本高于 trait 实现  
  Mitigation: 当前项目 feature 本来就是闭集，显式修改枚举正是为了保证边界清晰和可审计。

- [Risk] 预加载 feature-owned toolsets 可能让“显式 `load_toolset`”和“初始化自动可见”两条路径并存  
  Mitigation: 在 spec 中明确只有 feature-owned toolsets 允许初始化预加载，其他可选 toolset 仍走显式加载；实现中补齐调试日志。

- [Risk] 线程迁移到不同宿主后，初始化时记录的环境快照可能陈旧  
  Mitigation: 首版接受单宿主/稳定宿主假设；若未来存在线程跨宿主迁移，再补显式重建环境快照的流程。

- [Risk] channel/user feature 解析若落在多个入口，容易再次分散  
  Mitigation: 只保留一个 `FeatureResolver` 入口，并对解析结果、默认回退和最终启用 feature 集合添加关键日志。

- [Risk] 一旦把 `enabled_features` 持久化，调整全局默认配置后旧线程不会自动跟随  
  Mitigation: 明确把全局配置定义为“新线程默认值来源”；若需要让旧线程同步新配置，走显式 rebase/migration 流程，而不是隐式漂移。

## Migration Plan

1. 新增 `Feature` / `Features` 模型和统一的 feature 解析入口，先提供“显式配置优先、未配置默认全开”的行为。
2. 扩展线程持久化状态，新增 `enabled_features` 并收编 `auto_compact_override` 到 `Feature::AutoCompact`。
3. 用 enum 驱动的初始化逻辑替换当前 `FeaturePromptProvider` 主路径，保留必要兼容层直到调用点迁移完成。
4. 在初始化链路中加入运行时环境感知构造步骤，并把结果写入稳定线程前缀。
5. 将 `Memory`、`Skill`、`AutoCompact` 等现有 feature 逐个迁移到枚举分支，补齐其 usage prompt、工具启用和状态初始化。
6. 调整工具可见性投影，使 feature-owned toolsets 可以在初始化后首轮可见，同时不影响显式 `load_toolset` 语义。
7. 补齐线程初始化、feature 持久化恢复、环境感知、工具预加载和日志相关测试。

Rollback strategy:

- 若 enum 驱动迁移过程中风险过高，可短期保留 provider 到 enum 的兼容适配层，由 enum 统一决定启用哪些 provider；但 `Feature` 解析和环境感知注入仍保留，避免回退到完全分散的模型。

## Open Questions

- `Feature::Skill` 首版是否只负责 skill catalog prompt，还是也要绑定后续 skill 执行相关工具或运行时入口。
- 运行时环境感知是否需要把当前工作目录、路径分隔符等更细粒度事实纳入稳定 prompt，还是先只保留 OS + shell。
- channel/user feature 配置最终落在哪一层配置结构里，是否需要支持通配用户或 channel 默认值。
