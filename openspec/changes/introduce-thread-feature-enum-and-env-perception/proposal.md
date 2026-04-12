## Why

当前线程初始化链路已经有稳定 prompt 注入能力，但 feature 注入仍通过 `FeaturePromptProvider` 这类行为抽象来组织。对当前项目来说，feature 集合本身是闭集，继续用 provider trait 会把本来简单的“按枚举启用 prompt 和工具”问题做成运行时抽象，同时也让 channel/user 维度的 feature 配置难以直接映射到线程初始化。

另一个缺口是系统还没有把当前操作系统和 shell 环境显式注入给模型。模型在调用 `bash`、命令会话或其他依赖宿主环境的工具时，缺少 Windows/Linux、默认 shell、路径风格等事实输入，容易产出错误命令或错误假设。

## What Changes

- 引入闭集 `Feature` 枚举与 `Features` 集合，用于表达线程初始化时启用的能力+工具，例如 `Memory`、`Skill`、`AutoCompact` 等。
- 新增 channel/user 到 `Features` 的解析流程；当前开发阶段默认全部 feature 开启，但配置入口和解析边界先固定下来。
- 将线程最终生效的 `Features` 持久化到 thread state，作为线程恢复后的 feature 真相；`channel/user` 配置只负责为新线程提供默认值。
- 线程初始化改为基于 `Features` 顺序化写入稳定 `System` 前缀，并按 feature 启用其附带工具集或运行时能力，而不是依赖统一的外部 `FeaturePromptProvider` trait/registry。
- 将现有 `auto_compact_override` 收编到持久化的 `Feature::AutoCompact` 语义中，不再保留单独的线程级 auto-compact override 字段。
- 为每个 feature 定义显式的注入职责：稳定 usage prompt、可选工具集、以及必要的线程状态初始化。
- 新增运行时环境感知能力，在初始化阶段向线程注入当前操作系统、shell 类型和相关执行环境事实，供模型稳定决策。
- **BREAKING**: 线程 feature 注入模型从 provider trait 驱动改为 `Feature` 枚举驱动；后续新增外部 feature 时需要显式扩展枚举和注入逻辑，而不是实现通用 provider contract。

## Capabilities

### New Capabilities
- `runtime-environment-perception`: 在线程初始化时向模型注入当前操作系统、shell 与执行环境事实。

### Modified Capabilities
- `thread-context-runtime`: 线程初始化与稳定 prompt 注入改为基于 `Feature` 枚举和 channel/user feature 解析结果执行，不再要求统一的 `FeaturePromptProvider` contract。
- `thread-managed-toolsets`: 线程初始化允许根据已启用 feature 预加载其附带工具集，使 feature-owned tools 在首轮请求中即可可见。

## Impact

- Affected code: `src/agent/feature/**`、`src/thread.rs`、`src/agent/worker.rs`、`src/config.rs`、可能新增的环境感知模块，以及对应测试。
- API impact: 需要新增 `Feature` / `Features` 类型、feature 解析入口和环境感知构造入口；线程持久化 state 需要保存 `enabled_features`，并移除独立的 `auto_compact_override` 语义；若 `FeaturePromptProvider` 已成为公共接口，则会被替换或降级为内部兼容层。
- Runtime impact: 线程初始化前缀将变得更稳定可审计，并在首轮请求中携带宿主 OS/shell 事实；部分 feature 关联工具会在初始化时直接进入线程可见工具集；线程恢复后继续以持久化 feature flags 为准，而不是重新依赖外部配置漂移。
