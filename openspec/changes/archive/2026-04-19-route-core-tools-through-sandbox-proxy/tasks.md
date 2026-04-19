## 1. Sandbox 注入与 JSON-RPC 扩展

- [x] 1.1 让 `ToolRegistry` / `ToolCallContext` 能携带当前 worker 安装的 sandbox 实例
- [x] 1.2 扩展 `src/agent/sandbox.rs` 的 JSON-RPC 协议，新增 command session 所需请求/响应模型
- [x] 1.3 在 sandbox proxy 内复用 `CommandSessionManager`，实现 `command.exec`、`command.write_stdin`、`command.list_unread_tasks`

## 2. Core Tool 路由

- [x] 2.1 将 `read`、`write`、`edit` 改为在 sandbox 开启时通过 sandbox 文件原语执行
- [x] 2.2 将 `exec_command`、`write_stdin`、`list_unread_command_tasks` 改为在 sandbox 开启时通过 sandbox command RPC 执行
- [x] 2.3 保持 sandbox 关闭时的宿主机执行语义和现有返回结构不变

## 3. 测试与回归

- [x] 3.1 为 sandbox 开启时的文件工具路由补充单元测试与 Bubblewrap 集成测试
- [x] 3.2 为 sandbox 开启时的 command session 路由补充单元测试与 Bubblewrap 集成测试
- [x] 3.3 运行相关测试与全量 `cargo test`，完成后更新任务状态
