# ARIA / AI Snapshot 观测目录

这个目录用于沉淀真实浏览器访问后的 ARIA snapshot、公开 AI snapshot、截图、元信息，以及与持久化状态相关的观测说明。

当前包含以下观测：

- [baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu)
  百度首页旧 `aria_snapshot` 采集。
- [ai-baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu)
  百度首页公开 `ariaSnapshot({ mode: 'ai' })` 采集与结构化解析。
- [bilibili-persist](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/bilibili-persist)
  B 站首页 + cookies 持久化复用验证。
- [rust-snapshot-compare](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/rust-snapshot-compare)
  基于真实百度样本，对照 Rust 侧 `snapshot` 与旧 `aria_snapshot` 的 API 和产物差异。
- [rust-snapshot-diff.md](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/rust-snapshot-diff.md)
  Rust 侧普通 `snapshot` 与旧 `aria_snapshot` 的差异说明。

通用约定：

- 每次脚本执行都会在各自目录下的 `runs/` 新建一个运行目录。
- 需要跨运行复用的 Browser 持久化状态文件统一放在 `.openjarvis/browser/`。
- 当前首版实现落盘的是 cookies-only 状态文件，不包含 `localStorage`、`sessionStorage`、`IndexedDB`；推荐按站点或用途再分子目录，例如 `.openjarvis/browser/bilibili-persist/browser-cookies.json`。
- `page-metadata.json` 记录一次运行的标题、URL、时间和关联产物。
- Rust 侧观测脚本会同时落 `browser-snapshot.txt` 与 `aria-snapshot.yaml` 两种快照。
- 新增的公开 AI snapshot observation 直接记录 `ariaSnapshot({ mode: 'ai' })` 的原始输出和结构化解析结果；它和当前 Rust `aria_snapshot` 还不是同一条实现链路。
