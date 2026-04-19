# 百度首页公开 AI Snapshot 观测

这个目录记录通过 Playwright 公开 API `locator('body').ariaSnapshot({ mode: 'ai' })` 直接采集到的百度首页 AI snapshot。它和当前仓库里 Rust `BrowserSessionManager::aria_snapshot(...)` 的旧观察不是同一条实现路径，这里专门用来回答“公开 AI snapshot 现在实际长什么样”。

## 本次样本

- 样本运行目录: [capture-20260419T033939.754Z](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z)
- 元信息: [page-metadata.json](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/page-metadata.json)
- 原始 AI snapshot: [ai-snapshot.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/ai-snapshot.yaml)
- 结构化 YAML: [ai-snapshot-structured.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/ai-snapshot-structured.yaml)
- 截图: [baidu-homepage.png](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/baidu-homepage.png)

## 采集方式

- 浏览器: `/usr/bin/chromium-browser`
- Playwright 调用: `locator('body').ariaSnapshot({ mode: 'ai' })`
- Playwright 版本: `1.59.1`
- 结构化解析: [browser_ai_snapshot_parse.rs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/scripts/browser_ai_snapshot_parse.rs)

这次不是通过当前 Rust browser sidecar 采集，而是直接调用公开 Playwright API，然后再用新脚本把原始文本转换成稳定 YAML AST。

## 快速结论

- 公开 `mode='ai'` 输出已经明显比旧 `ariaSnapshot()` 更贴近 agent 读取：
  - 顶层会直接给出 `link`、`textbox`、`button`、`list`、`listitem`
  - 链接会把 URL 作为 `/url` 属性节点挂在下面
  - 普通文本会显式落成 `text`
- 这次百度样本里已经稳定出现 `[ref=...]`
- 这次百度样本里也出现了额外状态属性，例如 `[cursor=pointer]`、`[active]`
- 结构化解析后的概念汇总是：
  - `roles`: `button`、`generic`、`img`、`link`、`list`、`listitem`、`textbox`
  - `properties`: `url`
  - `attributes`: `active`、`cursor`、`ref`

## 和旧观察最不一样的点

相对当前仓库里的旧 `aria_snapshot` 观察，这次公开 AI snapshot 有两个需要特别注意的事实：

- 当前样本有 `ref`
  这说明在支持 `mode='ai'` 的 Playwright 版本里，公开 AI snapshot 已经可以携带元素引用，而不是纯语义树。
- 当前样本的 iframe 行为更保守
  在本地最小静态页验证里，`ariaSnapshot({ mode: 'ai' })` 会输出 `iframe [ref=...]`，并继续展开子树。

最小静态页验证摘录：

```yaml
- main [ref=e2]:
  - heading "Example" [level=1] [ref=e3]
  - button "Search" [ref=e4]: Go
  - textbox "Type here" [ref=e5]
  - iframe [ref=e6]:
    - button "Inside" [ref=f1e2]
```

所以，如果后续实现严格按公开 API 走，当前 observation 更支持这样的判断：

- 公开 AI snapshot 适合做语义理解、结构判断、规则抽取
- 它已经具备 `ref`、状态属性和 iframe 子树，和现有扁平 `snapshot/ref/act` 的能力边界开始明显靠近

## 关于旧误判

这个目录更早一份 2026-04-19 的样本是在 Playwright `1.55.1` 下采的。当时仓库版本还不支持公开 `mode='ai'` 参数，调用会退回到默认 `ariaSnapshot()` 行为，所以会错误地表现成“没有 `ref`”。本次样本升级到 `1.59.1` 后，这个问题已经消失。

## 产物规模

- [ai-snapshot.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/ai-snapshot.yaml) 为 120 行
- [ai-snapshot-structured.yaml](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/ai-baidu/runs/capture-20260419T033939.754Z/ai-snapshot-structured.yaml) 为 611 行

原始文本适合直接观察 Playwright 返回格式；结构化 YAML 更适合后续程序消费。

## 复现思路

1. 用 Playwright 打开百度首页
2. 执行 `locator('body').ariaSnapshot({ mode: 'ai' })`
3. 把返回文本保存成 `ai-snapshot.yaml`
4. 用 `cargo run --bin browser_ai_snapshot_parse -- --input <raw> --output <structured>` 生成结构化 YAML
