## 1. 配置、协议与会话边界扩展

- [x] 1.1 扩展 `src/config.rs` 与 `src/agent/tool/browser/protocol.rs`，新增 browser cookies 状态文件路径、自动注入/自动导出 flag，以及统一 `browser__open(mode=launch|attach)` 的请求/响应类型
- [x] 1.2 扩展 `src/agent/tool/browser/service.rs` 与 `src/agent/tool/browser/session.rs`，为统一 open、close 自动导出和手动 cookies 导出提供统一的 session 级调用入口

## 2. Sidecar 会话生命周期与状态复用

- [x] 2.1 在 `scripts/browser_sidecar.mjs` 中实现统一 open 路径，支持 launch 与显式 CDP attach 两种模式，并保证 attach 失败时不回退到 launch 模式
- [x] 2.2 在 `scripts/browser_sidecar.mjs` 中实现 launch 模式下的 cookies 自动注入、close 自动导出与显式手动导出
- [x] 2.3 区分 launch 模式与 attach 模式的 close 语义，确保 attach 模式下不会关闭外部浏览器进程，并把导出摘要或状态纳入 close 结果

## 3. Browser Toolset、Command 与 Helper 接入

- [x] 3.1 在 `src/agent/tool/browser/tool.rs` 中新增 `browser__open`，并让默认 lazy session 初始化复用同一 open 语义，移除对独立 cookies 工具 / 独立 attach 动作的接口依赖
- [x] 3.2 在 `src/command.rs` 与 hidden browser helper / script 路径中新增手动 cookies 导出入口，并支持通过 open 参数验证 launch / attach 两条主路径

## 4. 测试与回归验证

- [x] 4.1 在 `tests/agent/tool/browser/` 与对应配置测试中补充 protocol、service、tool 单元测试，覆盖自动 cookies 注入/导出、缺失状态文件、attach 错误和 session 替换生命周期
- [x] 4.2 增加 Command / helper / smoke 验证，覆盖“首次登录后 close 自动导出 -> 新会话 open 自动注入复用”以及“`browser__open(mode=attach)` 到已有 endpoint 并正常 close”两条主路径
