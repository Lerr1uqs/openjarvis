## Context

当前仓库已经具备线程级 toolset 管理能力，可以把非基础工具注册成按线程加载的工具集，但还没有浏览器能力的具体落点。结合前序调研，浏览器能力的最终方向不是 Rust 直接暴露底层 CDP 或 selector API，而是让 Rust 侧承担调度与工具封装，让 Node Playwright sidecar 承担真实浏览器控制。

这次变更的约束比较明确：

- 用户希望实现形式贴近最终形态，因此接受引入 Node sidecar 和 Playwright 依赖。
- 用户希望先以单一模块的形式放到 `agent/tool` 下面试跑，不接入现有 router、session、AgentWorker 主流程。
- 现有仓库已经有隐藏内部 helper 的模式可复用，例如 `internal-mcp`，因此浏览器能力也适合先提供内部验证入口，再决定是否并入正式运行面。
- 现阶段只需要跑通最小闭环，不需要一次性覆盖下载、上传、多 tab、登录态复用、trace 归档等完整浏览器平台能力。

## Goals / Non-Goals

**Goals:**
- 在 `src/agent/tool/` 下面新增一个可独立注册的 `browser` toolset 原型。
- 让 Rust 侧通过 Node Playwright sidecar 控制 Chrome，验证 `tool -> sidecar -> browser` 真实链路。
- 首版工具接口围绕 `snapshot/ref/act` 设计，并支持最小可用动作集合。
- 在不接入主流程的前提下，提供隐藏 CLI helper 和真实链路 smoke 验证入口。
- 保持模块边界清晰，方便后续把浏览器能力抽到更通用的 `src/browser/` 层或接入 MCP/browser server。

**Non-Goals:**
- 本次不接入现有 router、session 持久化和 AgentWorker 默认启动流程。
- 本次不实现浏览器账号态复用、长期 profile 持久化和多用户共享浏览器实例。
- 本次不直接把 sidecar 做成 MCP server，也不修改现有 MCP 管理协议。
- 本次不实现完整网页自动化能力全集，首版只覆盖最小 observe-act 闭环。
- 本次不把浏览器产物归档接入线程持久化，只保留本地临时产物用于调试和 smoke 验证。

## Decisions

### 1. 浏览器能力先落在 `src/agent/tool/browser/`，但内部按独立子模块分层

本次仍然遵从用户“先放到 `agent/tool` 下面试试看”的要求，不单独引入顶层 `src/browser/` 模块；但在目录内部按独立职责拆分：

- `mod.rs`: 导出 browser toolset 入口
- `protocol.rs`: 定义 Rust 与 Node sidecar 的请求/响应协议
- `service.rs`: 管理 sidecar 进程、stdio 通信、健康检查和错误包装
- `session.rs`: 管理单次浏览器会话、临时目录和生命周期
- `tool.rs`: 暴露 `browser__*` 工具处理器

这样既符合当前仓库的 tool 模块组织方式，也给后续抽取为独立浏览器子系统保留清晰迁移路径。

Alternative considered:
- 现在就创建顶层 `src/browser/` 并把 `agent/tool` 只当成很薄的 adapter。
  Rejected，因为当前目标是先做单模块原型并减少对现有代码面的扰动；过早重构顶层模块会扩大改动范围。

### 2. 浏览器执行层使用 Node Playwright sidecar，通过 stdio 结构化协议与 Rust 通信

Rust 首版不直接使用 `chromiumoxide` 或其他 Rust CDP crate，而是通过 `tokio::process` 拉起 Node sidecar，并用结构化 stdio 协议完成命令和结果交换。选择 stdio 的原因是：

- 贴近“Rust 调 sidecar”这一最终形态
- 不需要额外端口管理和本地 HTTP server 生命周期
- 与仓库现有隐藏 helper / stdio sidecar 模式一致
- 便于在测试里直接拉起并回收

Rust 侧对外暴露的是稳定的 tool API 和 service/session 抽象，而不是 sidecar 进程细节。未来如果要切换到 MCP browser server 或 HTTP sidecar，只需要替换 `service.rs` 这一层。

Alternative considered:
- Rust 直接使用 `chromiumoxide` 控制 Chrome。
  Rejected，因为这虽然更快，但偏离用户已经确认的最终方向，也会让后续切回 Playwright sidecar 时产生接口返工。

Alternative considered:
- sidecar 首版直接实现为 MCP server。
  Rejected，因为本次还不接现有程序运行面；先做定制 stdio 协议更容易收敛 scope，后续再决定是否挂到 MCP。

### 3. `browser` toolset 采用线程加载模型，并在一个线程内维护一个懒加载浏览器会话

本次浏览器能力仍然走现有 `thread-managed-toolsets` 机制，toolset 名称固定为 `browser`。在某个线程中加载该 toolset 后，模型可见的工具至少包括：

- `browser__navigate`
- `browser__snapshot`
- `browser__click_ref`
- `browser__type_ref`
- `browser__screenshot`
- `browser__close`

浏览器会话按线程维度维护，并采用懒加载策略：

- `load_toolset("browser")` 只让工具可见，不立即拉起浏览器
- 第一次真正调用浏览器工具时才创建 sidecar 和浏览器上下文
- `browser__close` 或 toolset 卸载后释放会话资源

这样可以避免“只加载工具但没有使用”时白白占用浏览器进程。

Alternative considered:
- 加载 `browser` toolset 时立即启动 sidecar 和浏览器。
  Rejected，因为启动成本更高，而且在用户只是探索工具可用性时会产生无谓开销。

### 4. 工具接口围绕 `snapshot/ref/act`，而不是 raw HTML + selector

首版接口采用调研文档推荐的 `snapshot/ref/act` 抽象：

- `browser__snapshot` 返回文本化页面快照和元素 `ref`
- `browser__click_ref`、`browser__type_ref` 使用 `ref` 驱动动作
- `browser__screenshot` 提供截图产物用于校验

sidecar 内部可以使用 Playwright locator、accessibility tree 或裁剪后的 DOM 信息构造 snapshot，但 Rust 侧和模型侧不依赖 CSS selector 作为主接口。这样更贴近后续 Agent 化浏览器控制，也更利于未来接入更强的页面观察协议。

Alternative considered:
- 直接向模型暴露 selector 和原始 HTML。
  Rejected，因为页面噪声大、selector 脆弱，而且与调研文档确定的最终方向相违背。

### 5. 浏览器实例必须使用独立 `user-data-dir`，并且禁止复用默认 Chrome Profile

每个浏览器会话都创建独立临时目录，并在其中创建专用 `user-data-dir`。首版默认不复用用户本机默认 Chrome Profile，也不承诺保留登录态。sidecar 应优先连接/拉起本机 Chrome，并在无法定位 Chrome 时返回明确错误。

这既是 Chrome 远程调试安全策略变化后的必要约束，也是为了避免自动化原型误伤用户本机浏览器数据。

Alternative considered:
- 直接附着或复用系统默认 Chrome Profile。
  Rejected，因为存在安全和稳定性风险，也不符合调研结论。

### 6. 验证入口采用“隐藏 CLI helper + `#[ignore]` smoke test”双轨

本次不直接把浏览器 toolset 接进主程序默认运行面，而是提供两个验证入口：

- 隐藏 CLI helper：用于开发期手动联调、观察日志、查看产物
- `#[ignore]` smoke test：用于真实链路回归验证，默认不进入普通 `cargo test`

推荐 helper 形态为新的隐藏命名空间，例如：

- `openjarvis internal-browser sidecar`
- `openjarvis internal-browser smoke --url https://example.com`

而 smoke test 则直接通过 Rust browser service 拉起 Node sidecar，覆盖 `navigate -> snapshot -> screenshot -> close` 的最小闭环。

Alternative considered:
- 只有单元测试，不做手动 helper。
  Rejected，因为 sidecar 联调阶段需要直接观察日志和产物，单元测试对问题定位不够友好。

Alternative considered:
- 直接接入现有主流程做端到端验证。
  Rejected，因为当前需求明确要求先不接入现有程序。

## Risks / Trade-offs

- [Node sidecar 与 Rust 双栈协作会增加调试复杂度] -> 通过隐藏 helper 保留直接联调入口，并在协议层返回结构化错误。
- [Playwright 和本机 Chrome 环境差异可能导致 smoke 不稳定] -> 把 smoke test 标记为 `#[ignore]`，只在手动验证或 CI 专门环境中运行。
- [首版 snapshot 实现可能不够强] -> 首版只要求支持最小可用 ref/action 闭环，并保持协议可扩展。
- [未接入主流程会让首次成果看起来“还没上线”] -> 本次目标本来就是验证最终形态的最小模块，避免把实验性浏览器能力直接引入现有运行面。
- [stdio 自定义协议未来可能需要迁移到 MCP] -> 将 Rust 对外接口限定在 `service/session/tool` 抽象上，避免协议细节泄漏到业务层。

## Migration Plan

1. 新增 Node Playwright sidecar 脚本和 Node 依赖，但不改默认启动流程。
2. 在 Rust 侧新增 `browser` toolset、sidecar service/session/protocol 和隐藏 CLI helper。
3. 增加单元测试，验证 toolset 注册、协议解析、会话生命周期和错误处理。
4. 增加 `#[ignore]` smoke test，验证真实浏览器链路。
5. 手动通过 helper 完成本机联调后，再评估是否进入下一阶段 change，把浏览器能力正式接入 agent 主流程。

Rollback strategy:
- 直接移除本次新增的 browser toolset、sidecar helper 和 Node Playwright 依赖即可；由于本次不修改默认主流程，回滚风险较低。

## Open Questions

- 首版 sidecar snapshot 的具体生成策略是优先 accessibility tree、裁剪 DOM，还是两者组合；该问题会影响实现细节，但不阻塞当前 change 立项。
- 是否在下一阶段直接把 sidecar 升级为 MCP browser server；本次先保持接口兼容空间，不在当前 change 内定案。
