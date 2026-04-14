# 百度首页 ARIA Snapshot 观测

这个目录用于存放真实浏览器访问 `https://www.baidu.com/` 后采集到的中间观测结果，重点是页面的 ARIA snapshot。

## 本次采集结果

- 目标 URL: `https://www.baidu.com/`
- 最终 URL: `https://www.baidu.com/`
- 页面标题: `百度一下，你就知道`
- 采集时间: `2026-04-14T13:53:30.795Z`
- 浏览器: `/usr/bin/chromium-browser`
- 采集脚本: [capture_baidu_aria.mjs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/capture_baidu_aria.mjs)
- 历史运行目录: [runs/capture-20260414T135330795Z](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260414T135330795Z)

## 目录说明

- `runs/`
  每次脚本执行都会创建一个新的运行目录。
- [runs/capture-20260414T135330795Z/aria-snapshot.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260414T135330795Z/aria-snapshot.yaml)
  一次实际采集到的页面 ARIA 树快照。
- [runs/capture-20260414T135330795Z/page-metadata.json](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260414T135330795Z/page-metadata.json)
  该次采集的 URL、标题、时间、浏览器路径和关联产物文件名。
- [runs/capture-20260414T135330795Z/baidu-homepage.png](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/baidu/runs/capture-20260414T135330795Z/baidu-homepage.png)
  该次采集时的整页截图，便于和 ARIA 结构对照。

## 采集方法

脚本使用 Playwright 直接启动本机 Chromium，访问百度首页，等待页面主体可见后输出：

1. `body` 根节点的 ARIA snapshot
2. 当前页面标题与最终 URL
3. 整页截图

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
node observation/aria-snapshot/baidu/capture_baidu_aria.mjs
```

如果 Chromium 不在 `/usr/bin/chromium-browser`，可以覆盖环境变量：

```bash
OPENJARVIS_OBSERVE_CHROME_PATH=/path/to/chrome \
node observation/aria-snapshot/baidu/capture_baidu_aria.mjs
```
