## Why

当前 `browser` toolset 已经具备 `navigate / snapshot / click / type / screenshot / close` 这条最小 observe-act 闭环，也补了动态页面下的语义匹配动作；但一旦页面因为前端脚本报错、接口失败、重定向异常或登录态问题而行为失真，模型侧只能看到截图和快照，无法直接判断失败原因。` arch/chrome-browser-automation.md ` 已经把 `console / errors / requests` 列为下一阶段推荐观察能力，因此现在适合补齐这块规范。

## What Changes

- 为线程级浏览器会话增加诊断观测能力，采集并保留 console 消息、页面错误和网络请求摘要。
- 为 `browser` toolset 增加只读诊断工具，支持在当前线程内查询最近的 console、error 和 request 记录。
- 在保留 browser artifacts 的场景下，把诊断记录落到当前 session 目录，方便 smoke/helper 回放和排障。

## Capabilities

### New Capabilities

- `browser-diagnostics`: 为现有 browser toolset 增加线程级诊断观测与查询能力。

### Modified Capabilities

## Impact

- Affected systems: `src/agent/tool/browser/`、 `scripts/browser_sidecar.mjs` 、browser helper/smoke 验证链路、 `tests/agent/tool/browser/` 。
- Tooling/API: `browser` toolset 会新增只读诊断工具，当前线程可见工具列表会扩大。
- Runtime: 继续复用现有 Node Playwright sidecar 与 session artifact 目录，不引入新的顶层组件。
