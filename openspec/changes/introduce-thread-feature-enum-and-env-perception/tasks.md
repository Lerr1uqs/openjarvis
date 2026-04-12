## 1. Feature 模型与解析

- [x] 1.1 新增闭集 `Feature` / `Features` 模型，定义稳定顺序与基础辅助接口
- [x] 1.2 实现统一的 feature 解析入口，支持按 `channel + user` 解析启用 feature，并提供开发阶段“未配置默认全开”行为
- [x] 1.3 扩展线程持久化状态，新增 `enabled_features` 作为线程 feature 真相，并移除独立的 `auto_compact_override` 语义
- [x] 1.4 为 feature 解析结果、默认回退和最终启用集合补齐关键调试日志

## 2. 初始化注入链路重构

- [x] 2.1 用 enum 驱动的初始化物化逻辑替换当前 `FeaturePromptProvider` 主路径，保留必要兼容层直到调用点迁移完成
- [x] 2.2 为 `Memory`、`Skill`、`AutoCompact` 等现有 feature 分别实现稳定 `System` prompt、工具启用和线程状态初始化逻辑
- [x] 2.3 调整线程初始化顺序为“基础角色 -> 环境感知 -> feature 物化”，并确保首轮请求即可看到正确前缀
- [x] 2.4 将 `Feature::AutoCompact` 的启停逻辑改为直接增删持久化 feature flags，而不是维护独立 override 字段
- [x] 2.5 将 feature-owned toolset 预加载接入现有线程工具状态与可见性投影，保持显式 `load_toolset` / `unload_toolset` 语义不回退

## 3. 运行时环境感知

- [x] 3.1 新增运行时环境感知构造器，采集 OS family、默认 shell/命令执行 shell 等最小必要事实
- [x] 3.2 为无法可靠探测的环境字段实现 `unknown` 回退，并补齐环境感知构建日志
- [x] 3.3 把环境感知 prompt 写入线程初始化 `System` 前缀，确保其直接作为持久化消息序列的一部分存在

## 4. 测试与回归验证

- [x] 4.1 在 `tests/` 下补齐 feature 解析与稳定顺序 UT，覆盖显式配置与默认全开两条路径
- [x] 4.2 补齐线程初始化与恢复 UT，验证 `enabled_features` 持久化恢复、feature prompt 注入顺序、feature-owned toolset 首轮可见性，以及 `AutoCompact` 状态初始化
- [x] 4.3 补齐环境感知 UT，覆盖已知 OS/shell、未知 shell 回退和线程前缀持久化行为
- [x] 4.4 补齐回归测试，确保现有 toolset 显式加载/卸载语义与 memory/compact 行为不被 feature 枚举化破坏
