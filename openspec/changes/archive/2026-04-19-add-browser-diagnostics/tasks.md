## 1. 协议与数据模型

- [x] 1.1 扩展 `src/agent/tool/browser/protocol.rs`，新增 console、errors、requests 诊断查询动作及其规范化结果类型
- [x] 1.2 扩展 `src/agent/tool/browser/service.rs` 与 `src/agent/tool/browser/session.rs`，为诊断查询提供统一的 session 级调用入口与参数透传

## 2. Sidecar 诊断采集

- [x] 2.1 在 `scripts/browser_sidecar.mjs` 中为 browser context / page 注册 console、pageerror、request、response、requestfailed 监听，并维护有界诊断缓冲区
- [x] 2.2 在保留 artifacts 的运行模式下，把诊断记录追加写入 session 目录中的 `console.jsonl`、`errors.jsonl` 和 `requests.jsonl`

## 3. Browser Toolset 接入

- [x] 3.1 在 `src/agent/tool/browser/tool.rs` 中新增 `browser__console`、`browser__errors`、`browser__requests` 工具定义、参数解析和结果渲染
- [x] 3.2 更新 browser toolset 注册与 mock sidecar/helper 行为，确保诊断工具可见性、查询路径和 session 清理语义一致

## 4. 测试与回归验证

- [x] 4.1 在 `tests/agent/tool/browser/` 下补充 protocol、service、tool 对应单元测试，覆盖诊断查询、限制参数、失败过滤和工具可见性
- [x] 4.2 补充 helper 或 artifact 相关测试，覆盖保留 session 目录时诊断文件落盘与关闭后的可读取行为
