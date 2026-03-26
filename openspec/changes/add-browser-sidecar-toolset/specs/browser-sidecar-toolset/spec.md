## ADDED Requirements

### Requirement: 系统 SHALL 提供可线程加载的 `browser` 浏览器工具集
系统 SHALL 以 program-defined toolset 的形式注册一个名为 `browser` 的浏览器工具集。该工具集在未被当前线程加载前 SHALL NOT 暴露浏览器动作工具；在当前线程加载后 SHALL 至少暴露稳定且无冲突的工具名：`browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref`、`browser__screenshot`、`browser__close`。

#### Scenario: 加载后浏览器工具可见
- **WHEN** 当前线程成功调用 `load_toolset` 加载 `browser`
- **THEN** 当前线程后续可见工具列表中包含 `browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref`、`browser__screenshot` 和 `browser__close`
- **THEN** 未加载 `browser` 的其他线程不会看到这些工具

### Requirement: 系统 SHALL 通过 Node Playwright sidecar 执行浏览器动作
系统 SHALL 通过独立的 Node Playwright sidecar 执行首版浏览器动作，Rust 工具层 SHALL 负责 sidecar 生命周期管理、协议封装和错误传播，而 SHALL NOT 在本次变更中直接使用 Rust 原生 CDP 库作为默认执行路径。

#### Scenario: 浏览器工具调用委托到 sidecar
- **WHEN** 当前线程第一次调用任意 `browser__*` 工具
- **THEN** 系统会按需启动一个 Node Playwright sidecar 并通过结构化协议发送浏览器动作请求
- **THEN** sidecar 返回的成功结果或失败错误会被 Rust 工具层规范化后返回给调用方

### Requirement: 系统 SHALL 使用 `snapshot/ref/act` 作为首版浏览器交互协议
系统 SHALL 以 `snapshot/ref/act` 作为首版浏览器交互协议。`browser__snapshot` SHALL 返回可供后续动作引用的页面文本快照和元素 `ref`；`browser__click_ref` 与 `browser__type_ref` SHALL 基于已有 `ref` 执行动作，而不是要求调用方直接提供 CSS selector 作为默认接口。

#### Scenario: 通过 snapshot 和 ref 完成页面动作
- **WHEN** 调用方先执行 `browser__navigate` 打开页面，再执行 `browser__snapshot`
- **THEN** `browser__snapshot` 的结果中包含可交互元素的 `ref`
- **THEN** 调用方可以将这些 `ref` 用于后续 `browser__click_ref` 或 `browser__type_ref`

### Requirement: 系统 SHALL 为每个浏览器会话使用独立的 Chrome 用户目录
系统 SHALL 为每个浏览器会话创建独立的临时目录和 `user-data-dir`，并 SHALL NOT 复用系统默认 Chrome Profile。会话关闭后，系统 SHALL 释放 sidecar 和浏览器资源，并清理本次运行创建的临时目录或将其作为调试产物显式保留。

#### Scenario: 浏览器会话不复用默认用户 Profile
- **WHEN** 系统为某个线程创建新的浏览器会话
- **THEN** 该会话使用的是当前运行新建的独立 `user-data-dir`
- **THEN** 系统默认 Chrome Profile 不会被直接附着或复用

### Requirement: 系统 SHALL 提供脱离主流程的独立验证入口
在未接入现有 router、session 和 AgentWorker 主流程的前提下，系统 SHALL 提供一个隐藏内部 CLI helper 用于手动验证浏览器 sidecar 链路，并 SHALL 提供一条真实浏览器链路的 smoke 验证路径用于回归。

#### Scenario: 开发者可独立运行浏览器验证链路
- **WHEN** 开发者执行隐藏内部 CLI helper 或手动触发 smoke 验证
- **THEN** 系统可以在不启动现有主程序消息处理流程的情况下完成 sidecar 启动、页面导航和基础浏览器动作验证
- **THEN** 验证结果会明确给出成功或失败信号以及必要的调试信息
