# Rust 侧 Snapshot 对照观测

这份文档对照的是当前 Rust `snapshot` 与旧 `aria_snapshot`。如果你要看公开 `ariaSnapshot({ mode: 'ai' })` 的新观察，请转到 [ai-baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu)。

这个 observation 基于同一次 Rust 采集结果，对比 `BrowserSessionManager::snapshot(...)` 和 `BrowserSessionManager::aria_snapshot(...)` 两条观测链路的差异。目标不是再定义一版 spec，而是直接回答“同一页为什么会落出两种 snapshot，它们各自给 agent 什么信息”。

## 对照样本

- 样本运行目录: [capture-20260415T143037.689Z](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z)
- 样本元信息: [page-metadata.json](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z/page-metadata.json)
- 普通快照: [browser-snapshot.txt](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z/browser-snapshot.txt)
- ARIA 快照: [aria-snapshot.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260415T143037.689Z/aria-snapshot.yaml)
- 采集脚本: [browser_baidu_aria_observe.rs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/scripts/browser_baidu_aria_observe.rs)

这次样本来自真实 Rust 调用链：Rust 入口负责驱动 `BrowserSessionManager`，随后分别调用 `snapshot` 和 `aria_snapshot`，再把两类产物一并落盘。

## Rust API 层差异

- `snapshot`
  - 调用入口: `BrowserSessionManager::snapshot(thread_id, Some(max_elements))`
  - 返回类型: `BrowserSnapshotResult`
  - 关键字段: `snapshot_text`、`elements`、`total_candidate_count`、`truncated`
  - 作用: 给后续 `click_ref` / `type_ref` 提供可执行的 `ref`
- `aria_snapshot`
  - 调用入口: `BrowserSessionManager::aria_snapshot(thread_id)`
  - 返回类型: `BrowserAriaSnapshotResult`
  - 关键字段: `aria_snapshot`
  - 作用: 保留辅助功能树的语义层级，不承担动作引用

从 Rust 类型上看，两者最核心的区别是：

- `snapshot` 有结构化 `elements` 列表，里面包含 `ref`、`selector`、`role`、`href` 等后续动作真正会用到的数据
- `aria_snapshot` 只有一份语义树字符串，没有 `ref`，也没有元素截断计数
- 只有 `snapshot` 支持 `max_elements` 这种面向自动化的截断控制

## 真实产物层差异

同一页上，这两类输出表达的信息密度不同：

- `browser-snapshot.txt`
  - 先给 URL、Title 和一段页面文本摘要
  - 再按 `[ref] role label -> href` 的形式线性列出可交互节点
  - 会保留一些自动化视角下有用但语义较弱的节点，例如 `div`、`svg`、`path`
- `aria-snapshot.yaml`
  - 直接输出辅助功能树
  - 用缩进表达 `list -> listitem -> link` 这种层级关系
  - 更强调 `textbox`、`button`、`link`、`list` 这些可访问语义

这次百度样本里：

- `browser-snapshot.txt` 为 85 行
- `aria-snapshot.yaml` 为 78 行
- 两者磁盘大小都约 8 KB，但这不代表信息等价，只说明这次页面规模相近

## 同页摘录

`snapshot` 摘录：

```text
URL: https://www.baidu.com/
Title: 百度一下，你就知道
[13] textbox 王阳被曝曾在剧组被导演霸凌
[14] button 百度一下
[48] link 习近平同苏林共同会见中越青年代表 -> https://www.baidu.com/s?wd=%E4%B9%A0%E8%BF%91%E5%B9%B3%E5%90%8C%E8%8B%8F%E6%9E%97%E5%85%B1%E5%90%8C%E4%BC%9A%E8%A7%81%E4%B8%AD%E8%B6%8A%E9%9D%92%E5%B9%B4%E4%BB%A3%E8%A1%A8&sa=fyb_n_homepage&rsv_dl=fyb_n_homepage&from=super&cl=3&tn=baidutop10&fr=top1000&rsv_idx=2&hisfilter=1
```

`aria_snapshot` 摘录：

```yaml
- textbox "王阳被曝曾在剧组被导演霸凌"
- button "百度一下"
- list:
  - listitem:
    - link " 习近平同苏林共同会见中越青年代表":
      - /url: https://www.baidu.com/s?wd=%E4%B9%A0%E8%BF%91%E5%B9%B3%E5%90%8C%E8%8B%8F%E6%9E%97%E5%85%B1%E5%90%8C%E4%BC%9A%E8%A7%81%E4%B8%AD%E8%B6%8A%E9%9D%92%E5%B9%B4%E4%BB%A3%E8%A1%A8&sa=fyb_n_homepage&rsv_dl=fyb_n_homepage&from=super&cl=3&tn=baidutop10&fr=top1000&rsv_idx=2&hisfilter=1
```

从这组摘录可以直接看到：

- `snapshot` 把页面扁平化后编号，方便 agent 拿 `ref=13`、`ref=14` 继续动作
- `aria_snapshot` 保留“热搜榜是一个 list，榜单项是 listitem，里面再挂 link”的语义层级
- 同样的搜索框和按钮，在 `aria_snapshot` 里仍然可见，但不再携带 `ref`

## 对 agent 的直接影响

- 需要点击、输入、按引用继续自动化时，优先看 `snapshot`
- 需要判断页面结构、榜单层级、登录区语义或做可访问性排查时，优先看 `aria_snapshot`
- 排查回归时最好同时保存两者
  - `snapshot` 变化大但 `aria_snapshot` 基本稳定，往往更像是 DOM 细节或候选元素筛选变了
  - `aria_snapshot` 也明显变化，通常说明页面语义结构本身变了
