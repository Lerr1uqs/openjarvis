## MODIFIED Requirements

### Requirement: 系统 SHALL 在线程初始化时将稳定 `System` 前缀直接写入 `Thread`
系统 SHALL 在创建或恢复某个线程并准备初始化时，先解析该线程当前启用的 `Features` 与运行时环境感知结果，再把基础 system prompt、运行时环境感知 prompt，以及所有已启用 feature 的稳定 usage prompt 直接写入 `Thread.messages()` 的 `System` 前缀。系统 SHALL NOT 为这些稳定前缀再维护独立的 request context 成员或 snapshot 字段。

参考初始化顺序如下：

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

#### Scenario: 新线程创建时按固定顺序初始化稳定前缀
- **WHEN** Session、Router 或同等线程派生入口首次为某个 internal thread 创建并初始化线程上下文
- **THEN** 系统会先解析该线程的 `Features` 与运行时环境感知结果
- **THEN** 该线程会同步写入自己的稳定 `System` 前缀
- **THEN** 该前缀中按稳定顺序包含基础 system prompt、环境感知 prompt 和已启用 feature 的稳定 prompt

## ADDED Requirements

### Requirement: 系统 SHALL 在线程初始化前解析当前线程的 `Features`
系统 SHALL 在新线程初始化前，根据 `channel + user` 维度配置解析当前线程启用的默认 `Features`。若存在显式配置，系统 SHALL 采用该配置结果；当前开发阶段若没有显式配置，系统 SHALL 默认启用全部已定义 feature。解析得到的 `Features` SHALL 被写入线程持久化状态，作为该线程后续恢复与运行时判断使用的正式 feature 真相。

#### Scenario: 命中显式 feature 配置时使用显式集合
- **WHEN** 某个 channel/user 组合存在显式 feature 配置，且仅启用了 `Memory` 与 `Skill`
- **THEN** 该线程初始化时解析得到的 `Features` 只包含 `Memory` 与 `Skill`
- **THEN** 这些 `Features` 会被持久化到该线程状态中
- **THEN** 未启用的 feature 不会参与该线程的初始化 prompt 与工具启用

#### Scenario: 开发阶段未配置时默认全部开启
- **WHEN** 某个 channel/user 组合没有显式 feature 配置
- **THEN** 系统会为该线程返回全部已定义 feature 的集合
- **THEN** 这些 feature 会在初始化时按统一顺序生效
- **THEN** 后续线程恢复时会继续使用已持久化集合，而不是重新依赖外部默认值求值

### Requirement: 系统 SHALL 通过闭集 `Feature` 枚举物化 feature prompt 与 feature-owned tools
系统 SHALL 使用闭集 `Feature` 枚举驱动线程初始化时的 feature 物化，而不是依赖统一的外部 `FeaturePromptProvider` contract。每个已启用 feature SHALL 显式声明自己的稳定 usage prompt、附带工具集或工具可见性输入，以及必要的线程状态初始化。

#### Scenario: `Memory` feature 在初始化时同时注入使用说明并启用附带工具集
- **WHEN** 某个线程初始化时启用了 `Feature::Memory`
- **THEN** 系统会把 memory 的稳定 usage prompt 写入该线程的 `System` 前缀
- **THEN** 系统会同步启用该 feature 附带的工具集或工具可见性输入
- **THEN** 该线程首轮请求即可看到该 feature 提供的稳定 prompt 与工具能力

#### Scenario: `AutoCompact` feature 只注入说明并设置可见性输入
- **WHEN** 某个线程初始化时启用了 `Feature::AutoCompact`
- **THEN** 系统会把 auto-compact 的稳定 usage prompt 写入该线程的 `System` 前缀
- **THEN** 系统会同步写入该 feature 对应的线程状态，并注入 `compact_tools()` 对应的工具可见性输入
- **THEN** 系统不会为了这个 feature 额外依赖统一 provider trait 才完成初始化

### Requirement: 系统 SHALL 以持久化的 `enabled_features` 作为线程 feature 真相
系统 SHALL 把线程最终生效的 `enabled_features` 持久化到 thread state，并在恢复、可见性判断和 feature prompt 物化时以该集合为准。系统 SHALL NOT 为 `AutoCompact` 再保留独立的 `auto_compact_override` 事实来源；`Feature::AutoCompact` 的存在本身就表示该线程启用了 auto-compact。

#### Scenario: 线程恢复后继续沿用持久化 feature 集合
- **WHEN** 某个线程已经持久化了 `enabled_features = {Memory, AutoCompact}`，随后从 store 恢复
- **THEN** 系统会直接使用这组持久化 features 进行后续请求组装
- **THEN** 线程不会因为外部 channel/user 默认配置变化而丢失 `AutoCompact`

#### Scenario: AutoCompact 不再依赖独立 override 字段
- **WHEN** 某个线程需要开启或关闭 auto-compact
- **THEN** 系统会通过增删 `Feature::AutoCompact` 修改该线程的 `enabled_features`
- **THEN** 系统不会再单独读写 `auto_compact_override`
