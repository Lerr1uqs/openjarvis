## ADDED Requirements

### Requirement: 系统 SHALL 支持初始化阶段的 feature-owned toolset 预加载
系统 SHALL 允许线程在初始化阶段根据已解析的 `Features` 预加载其附带的 toolset。被某个 feature 拥有并在初始化阶段启用的 toolset SHALL 在首轮模型请求中立即可见，而 SHALL NOT 要求模型先显式调用一次 `load_toolset` 才能使用。

#### Scenario: `Memory` feature 预加载 `memory` toolset
- **WHEN** 某个线程在初始化时启用了拥有 `memory` toolset 的 feature
- **THEN** 该线程首轮请求的 visible tools 中会包含 `memory` toolset 对应工具
- **THEN** 模型不需要先额外调用 `load_toolset(memory)` 才能使用这些工具
- **THEN** 其他未启用该 feature 的线程仍不会因此自动看到这些工具
