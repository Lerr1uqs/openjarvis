## Why

当前 OpenJarvis 已经具备线程级 toolset 装载机制，但还没有一个贴近最终形态的浏览器自动化能力落点。继续停留在架构调研阶段，会让后续的工具协议、sidecar 进程模型、测试入口和模块边界都缺少可运行验证。

这次变更需要先把浏览器能力做成一个独立可跑通的 toolset 原型，验证 `Rust tool -> Node sidecar -> Playwright -> Chrome` 这条链路，同时避免过早接入现有主流程，降低对当前 agent 运行面的扰动。

## What Changes

- 新增一个浏览器自动化 toolset 原型，放在 `src/agent/tool` 体系内，作为可独立注册和调用的非基础工具集。
- 引入 Node.js Playwright sidecar，负责实际浏览器控制；Rust 侧只负责协议封装、进程调度、错误处理和工具暴露。
- 首版工具接口围绕 `snapshot/ref/act` 建模，优先支持最小闭环动作：`navigate`、`snapshot`、`click_ref`、`type_ref`、`screenshot`、`close`。
- 新增隐藏的内部 CLI helper，用于手动拉起 sidecar 或执行单次 smoke 流程，在不接入现有 router/agent 主流程的前提下完成联调验证。
- 新增真实链路的 smoke 验证路径，覆盖 sidecar 启动、浏览器拉起、页面导航、页面观察和基础动作调用。
- 明确首版浏览器运行约束：使用独立 `user-data-dir`、不复用默认 Chrome Profile、不做账号态复用、不接入现有会话持久化。

## Capabilities

### New Capabilities
- `browser-sidecar-toolset`: 以线程可加载 toolset 的形式提供基于 Node Playwright sidecar 的浏览器自动化工具能力，并提供独立的运行与验证入口。

### Modified Capabilities

## Impact

- Affected code: `src/agent/tool/**`、`src/cli.rs`、`src/main.rs`、`src/lib.rs`，以及新增的 sidecar 脚本目录和对应测试。
- Affected dependencies: Rust 侧会增加 sidecar 进程调用和协议处理依赖；Node 侧会新增 `playwright` 依赖。
- Runtime impact: 首版不接入现有 router / session / AgentWorker 主流程，默认只通过内部 helper 和测试入口运行。
- API impact: tool registry 将新增一个可注册的浏览器 toolset，以及对应的结构化浏览器动作工具名。
- Verification impact: 需要补充单元测试与 `#[ignore]` smoke test，确保真实浏览器链路可以手动回归验证。
