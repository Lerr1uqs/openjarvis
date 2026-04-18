## 1. Kind Profile 建模

- [x] 1.1 为 `ThreadAgentKind` 新增统一 capability profile 解析入口，明确 system prompt、默认工具、允许 toolset 和允许 feature 的结构
- [x] 1.2 将现有 `Main`、`Browser` 的初始化 truth 收口到 capability profile，而不是分别散落在 prompt/tool/feature 初始化链路中
- [x] 1.3 明确 `Browser` 首版只保留浏览器职责，不启用 `memory`、`skill`、`subagent` 等 feature

## 2. 初始化与工具投影收口

- [x] 2.1 重构 `initialize_thread`，使其通过 `kind + capability profile` 统一决定稳定 prompt、默认工具和 feature 注入边界
- [x] 2.2 让 resolver 或持久化恢复得到的 feature 集合在初始化前先经过当前 kind profile 允许范围过滤
- [x] 2.3 重构 thread-scoped tool visibility、toolset catalog 与 `load_toolset` / `unload_toolset`，使其受当前 kind profile 的 allowed toolset 边界约束
- [x] 2.4 明确 `Main` 不直接暴露 `browser` toolset，浏览器能力统一通过 `subagent -> Browser` 路径使用

## 3. 测试与回归验证

- [x] 3.1 补齐主线程与 `Browser` 线程初始化测试，覆盖 prompt、默认工具和 feature 边界都来自 kind profile
- [x] 3.2 补齐工具可见性与 toolset catalog 测试，覆盖不同 kind 看到的可用工具和可加载 toolset 不同
- [x] 3.3 补齐回归测试，覆盖不允许的 feature/tool/toolset 不会在后续运行时重新暴露
- [x] 3.4 补齐主线程不能直接加载 `browser` toolset、但仍可通过 `subagent` 使用 `Browser` 的回归测试
