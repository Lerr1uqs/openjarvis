## 1. Node Sidecar 与依赖准备

- [x] 1.1 在 `package.json` 中引入 `playwright` 及所需脚本入口，并新增首版 browser sidecar 脚本文件
- [x] 1.2 为 Node sidecar 实现首版 stdio 结构化协议，覆盖 `navigate`、`snapshot`、`click_ref`、`type_ref`、`screenshot`、`close` 六个动作
- [x] 1.3 为 sidecar 实现 Chrome 启动、独立 `user-data-dir` 创建、临时目录管理和基础错误输出

## 2. Rust 浏览器 Toolset

- [x] 2.1 新增 `src/agent/tool/browser/` 模块结构，并实现 protocol、service、session、tool 的基础类型与边界
- [x] 2.2 实现 Rust 到 Node sidecar 的进程管理、stdio 通信、懒加载浏览器会话和结果规范化
- [x] 2.3 实现 `browser__navigate`、`browser__snapshot`、`browser__click_ref`、`browser__type_ref`、`browser__screenshot`、`browser__close` 工具，并将其注册为线程可加载的 `browser` toolset

## 3. 独立验证入口

- [x] 3.1 扩展 `src/cli.rs` 与 `src/main.rs`，新增隐藏的 `internal-browser` helper 命令空间
- [x] 3.2 实现手动 smoke 流程命令，完成 `navigate -> snapshot -> screenshot -> close` 的最小闭环并输出调试产物位置
- [x] 3.3 补充 Playwright 安装前置条件、helper 使用方式和本地手动验证说明

## 4. 测试与回归验证

- [x] 4.1 在 `tests/agent/tool/browser/` 下补齐对应单元测试，覆盖 toolset 注册、协议编解码、sidecar 生命周期和错误路径
- [x] 4.2 补充 `tests/cli.rs` 或对应测试文件，覆盖 `internal-browser` helper 的参数解析和独立运行入口
- [x] 4.3 新增真实浏览器链路的 `#[ignore]` smoke test，验证 sidecar 启动、页面导航、snapshot、screenshot 和显式关闭流程
