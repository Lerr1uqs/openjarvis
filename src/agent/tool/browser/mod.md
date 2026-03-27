# agent/tool/browser 模块总览

## 作用

`browser/` 提供线程级浏览器自动化能力。Rust 侧负责工具注册、会话管理、协议封装，实际浏览器驱动由 Node Playwright sidecar 执行。

## 子模块

- `protocol.rs`
  协议层。定义 Rust 与 sidecar 之间的 JSON Line 请求/响应格式。
- `service.rs`
  进程通信层。负责 sidecar 进程拉起、传输、响应收发。
- `session.rs`
  会话管理层。负责为每个 thread 管理隔离的浏览器 session 与落盘目录。
- `tool.rs`
  工具暴露层。负责把浏览器动作注册成 Agent 可调用的工具。

## 核心概念

- `Browser Sidecar`
  实际驱动浏览器的外部进程。Rust 不直接操纵浏览器，而是把动作委托给 sidecar。
- `BrowserSession`
  某个 thread 独享的一次浏览器会话，包含独立目录、状态与 sidecar 生命周期。
- `Snapshot`
  当前页面的结构化快照，不是原始 DOM 全量镜像，而是给模型决策用的可交互元素视图。
- `Ref`
  快照中元素的引用编号。后续 `click_ref`、`type_ref` 这类动作会依赖它。
- `BrowserToolsetRuntime`
  浏览器工具集在当前线程下的运行时对象，负责持有和复用 session。

## 常见动作语义

- `navigate`
  打开或跳转页面。
- `snapshot`
  获取当前页面可供模型理解和操作的结构化视图。
- `click_ref` / `type_ref`
  对已经定位好的元素执行动作。
- `click_match` / `type_match`
  先按条件匹配元素，再执行动作。
- `screenshot`
  产出当前页面截图。
- `close`
  显式关闭当前线程的浏览器会话。

## 边界

- 本模块关心“浏览器如何被线程安全地使用”，不关心高层任务规划。
- 本模块也不直接决定什么时候加载浏览器工具集，那是 `ToolRegistry` 的职责。
