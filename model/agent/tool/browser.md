# Browser Toolset

## 定位

- `browser` 是线程级浏览器自动化工具集。
- Rust 侧负责工具注册、会话管理和协议封装，真正驱动浏览器的是 Node Playwright sidecar。

## 边界

- 负责线程级浏览器 session、sidecar 通信、快照和页面动作。
- 不负责高层任务规划，不负责决定何时加载 browser toolset。

## 关键概念

- `BrowserSidecarService`
  Rust 和 Node sidecar 的进程通信层。
- `BrowserSessionManager`
  按线程管理浏览器 session。
- `BrowserToolsetRuntime`
  browser toolset 的线程运行时对象，卸载时负责清理 session。
- `Snapshot`
  面向模型的结构化页面视图，不是原始 DOM 全量镜像。
- `ref`
  快照中元素的稳定引用编号。

## 核心能力

- 为每个线程隔离浏览器会话和产物目录。
- 提供 `navigate / snapshot / click / type / screenshot / close` 等动作。
- 支持两种定位方式：直接用 `ref`，或按语义条件匹配元素。
- 卸载 toolset 时自动关闭当前线程浏览器资源。

## 使用方式

- browser 不是全局常驻工具，而是按线程加载的 toolset。
- 模型应先观察 `snapshot`，再基于 `ref` 或语义匹配执行动作。
