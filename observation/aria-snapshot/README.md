# ARIA Snapshot 观测目录

这个目录用于沉淀真实浏览器访问后的 ARIA snapshot、截图、元信息和持久化状态文件。

当前包含两类观测：

- [baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu)
  百度首页 ARIA snapshot 采集。
- [bilibili-persist](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/bilibili-persist)
  B 站首页 + cookies 持久化复用验证。

通用约定：

- 每次脚本执行都会在各自目录下的 `runs/` 新建一个运行目录。
- 持久化状态文件放在各自目录下的 `state/`。
- `page-metadata.json` 记录一次运行的标题、URL、时间和关联产物。
