## 1. 配置与运行时模型

- [ ] 1.1 扩展 `agent.tool` 配置，新增工具执行环境选择与 Bubblewrap 沙箱配置项
- [ ] 1.2 在 `AgentRuntime` 中引入统一执行层依赖，并把当前 placeholder sandbox 模型替换为真实运行时所有权
- [ ] 1.3 为执行环境初始化、失败路径和关键动作补充结构化日志

## 2. 执行层抽象与本地 backend

- [ ] 2.1 新增统一执行层接口，覆盖文件读写/替换与子进程启动/等待/回收等通用原语
- [ ] 2.2 实现 `Local` backend，保证现有宿主执行语义在统一执行层下可继续工作
- [ ] 2.3 为执行层结果、错误和路径映射补充单元测试与文档级示例

## 3. Bubblewrap helper 与沙箱 backend

- [ ] 3.1 新增隐藏的内部 helper 入口，实现基于 JSON Lines 的执行协议
- [ ] 3.2 实现 Bubblewrap backend，包括 helper 启动、挂载组装、工作区路径映射、环境清理和生命周期管理
- [ ] 3.3 实现 Bubblewrap 不可用、平台不支持、helper 启动失败等显式失败路径

## 4. 工具迁移到统一执行层

- [ ] 4.1 将 builtin `read`、`write`、`edit`、`bash` 迁移到统一执行层
- [ ] 4.2 将 memory toolset 的文件仓库访问迁移到统一执行层
- [ ] 4.3 将 browser sidecar 和 stdio MCP server 的工具自有子进程启动迁移到统一执行层

## 5. 验证与回归

- [ ] 5.1 在 `tests/agent/tool/` 和对应测试目录下补齐本地 backend、沙箱 backend、helper 协议和错误路径测试
- [ ] 5.2 新增 Linux-only 集成测试，验证 Bubblewrap helper 可启动、工作区路径可访问、未授权宿主路径会被阻止
- [ ] 5.3 新增回归测试，确保配置选择 `Sandbox` 但环境不支持时会显式失败而不是静默回退到本地执行
