# 百度首页 ARIA Snapshot 观测

如果你要看新的公开 AI snapshot 观察，请转到 [ai-baidu](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu)。当前这个目录仍然记录的是旧 Rust `aria_snapshot` 路径。

这个目录用于存放真实浏览器访问 `https://www.baidu.com/` 后采集到的中间观测结果，重点是页面的 ARIA snapshot。

## 本次采集结果

- 目标 URL: `https://www.baidu.com/`
- 最终 URL: 以最新一次 `runs/` 目录内的 `page-metadata.json` 为准
- 页面标题: 以最新一次 `runs/` 目录内的 `page-metadata.json` 为准
- 采集时间: 以最新一次 `runs/` 目录内的 `page-metadata.json` 为准
- 浏览器: `/usr/bin/chromium-browser`
- 采集脚本: [browser_baidu_aria_observe.rs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/scripts/browser_baidu_aria_observe.rs)

## 目录说明

- `runs/`
  每次脚本执行都会创建一个新的运行目录。
- `browser-snapshot.txt`
  Rust `BrowserSessionManager::snapshot(...)` 的文本快照，偏向可操作元素摘要。
- `aria-snapshot.yaml`
  Rust `BrowserSessionManager::aria_snapshot(...)` 的语义树快照，偏向辅助功能结构。
- `page-metadata.json`
  该次采集的 URL、标题、时间、浏览器路径和关联产物文件名。
- `baidu-homepage.png`
  该次采集时的整页截图，便于和两种快照对照。

## 采集方法

脚本通过 Rust `BrowserSessionManager` 启动浏览器，访问百度首页，随后输出：

1. 普通 `snapshot`
2. `body` 根节点的 `aria_snapshot`
3. 当前页面标题与最终 URL
4. 整页截图

这样可以把“视觉页面”和“辅助功能树”同时落盘，方便后续排查页面可访问性、元素语义和自动化定位问题。

## 快速观察

当前这份 ARIA snapshot 中可以直接看到这些结构：

- 顶部导航链接，例如 `新闻`、`贴吧`、`图片`
- 搜索输入框 `textbox`
- `百度一下` 搜索按钮
- `百度热搜` 区域及其 `list` / `listitem`
- 页脚中的备案、帮助、关于百度等链接

由于百度首页内容会动态变化，尤其是搜索框默认提示词和热搜列表，这份快照是一次具体采集时刻的结果，不应当被视为长期稳定基线。

## 复现命令

在仓库根目录执行：

```bash
cargo run --bin browser_baidu_aria_observe
```

如果 Chromium 不在 `/usr/bin/chromium-browser`，可以覆盖环境变量：

```bash
OPENJARVIS_BROWSER_CHROME_PATH=/path/to/chrome \
cargo run --bin browser_baidu_aria_observe -- --chrome-path /path/to/chrome
```
