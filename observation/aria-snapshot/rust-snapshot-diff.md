# Rust 侧两种 Snapshot 的差异

如果你要看新的公开 `ariaSnapshot({ mode: 'ai' })` observation，请转到 [ai-baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu)。当前这份说明仍然针对旧 Rust `aria_snapshot`。

更完整的 observation 见 [rust-snapshot-compare](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/rust-snapshot-compare)。

当前 Rust 侧浏览器观测链路可以产出两种不同的页面快照：

- `snapshot`
  通过 Rust `BrowserSessionManager::snapshot(...)` 调用 sidecar 的普通页面快照。
- `aria_snapshot`
  通过 Rust `BrowserSessionManager::aria_snapshot(...)` 调用 sidecar 的 ARIA 快照。

## 关注点不同

`snapshot` 更偏向自动化操作视角：

- 会返回 `ref`、`selector`、`tag_name`、`role`、`label`、`href` 等结构化元素信息
- 会把可交互元素整理成一份便于点击和输入的文本列表
- 输出里自带可直接用于后续动作的元素引用

`aria_snapshot` 更偏向可访问性语义视角：

- 保留的是页面辅助功能树的层级结构
- 更适合观察标题、列表、链接、按钮、文本框等语义关系
- 不提供 Rust/browser tool 里的 `ref` 编号，也不直接面向点击动作

## 典型产物差异

以百度首页为例，Rust 侧一次观测会同时落下：

- [browser-snapshot.txt](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z/browser-snapshot.txt)
  这里的内容更像“给 agent 下一步操作用的页面摘要”。
- [aria-snapshot.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z/aria-snapshot.yaml)
  这里的内容更像“页面辅助功能树的语义展开”。

当前引用的是 2026-04-15 这次真实 Rust 采集样本；后续如果重新采集，请以最新的 `runs/` 目录为准。

## 该如何选

如果你的目标是：

- 找按钮、找输入框、找链接并继续自动化操作：优先看 `snapshot`
- 判断页面当前的语义结构、辅助功能树、登录区块或内容层级：优先看 `aria_snapshot`

在真实排查里，这两种快照最好一起看：

- `snapshot` 告诉你“这个页面现在可操作什么”
- `aria_snapshot` 告诉你“这个页面现在被浏览器语义化成什么”
