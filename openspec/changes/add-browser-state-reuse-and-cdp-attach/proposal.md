## Why

当前 `browser` toolset 每次都会创建新的临时 `user-data-dir`，这虽然安全，但会让需要登录的网站在每次新会话里重复登录，也无法复用已经由用户手动打开的 Chromium 实例。对于真实 agent 浏览器任务，最常见的两个增量能力就是“通过显式状态文件复用 cookies”以及“通过显式 CDP endpoint attach 到已有 Chromium”，同时用户还希望把 launch / attach 的选择收敛到一个统一 open 入口里。

## What Changes

- 为 browser runtime 增加配置文件驱动的 cookies 状态复用能力，首版聚焦显式状态文件、open 自动注入和 close 自动导出。
- 为浏览器链路保留一个显式 Command / helper，用于把当前 session cookies 手动导出到指定路径。
- 为 browser toolset 增加统一的 `browser__open` 会话入口，通过参数在 `launch` 与 `attach` 两种模式之间做显式选择，而不是拆成独立 attach 动作。
- 为 helper / tool / session 边界补齐 cookies 生命周期、attach 模式约束、错误处理和验证路径。

## Capabilities

### New Capabilities

- `browser-cookie-state-persistence`: 为 browser 会话提供基于显式状态文件的 cookies 自动复用与手动导出能力。
- `browser-cdp-attach`: 为 browser 会话提供通过显式 CDP endpoint attach 到已有 Chromium 实例的能力。

### Modified Capabilities

- `browser-sidecar-toolset`: 为现有 browser toolset 增加统一的 `browser__open` 会话入口，并让 launch / attach / close 生命周期围绕同一套 open 语义收敛。

## Impact

- Affected systems: `src/agent/tool/browser/`、 `scripts/browser_sidecar.mjs` 、 `src/config.rs` 、 `src/command.rs` 、hidden browser helper、 `tests/agent/tool/browser/` 。
- Tooling/API: browser toolset 会新增 `browser__open`；cookies 复用改为配置文件 flag 驱动，并补充手动导出 Command / helper；attach 入口会并入 open 参数。
- Runtime: sidecar 将同时支持“启动新浏览器”和“attach 到现有浏览器”两条执行路径，并在 close 生命周期中按配置处理 cookies 自动导出。
