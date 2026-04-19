## 1. 配置与抽象

- [x] 1.1 新增 `config/capabilities.yaml` 的读取、默认值和校验逻辑
- [x] 1.2 将 `src/agent/sandbox.rs` 从占位类型重构为统一 `Sandbox` trait 和后端枚举
- [x] 1.3 为 Bubblewrap 与 Docker 建立统一工厂入口，并让 Docker 当前显式返回未实现错误

## 2. Bubblewrap JSON-RPC 运行时

- [x] 2.1 定义宿主与沙箱 proxy 之间的 JSON-RPC 请求/响应模型
- [x] 2.2 新增隐藏 `internal-sandbox` CLI/helper 入口，并实现 proxy 主循环
- [x] 2.3 实现 Bubblewrap 后端的 proxy 启动、生命周期管理和请求发送

## 3. 路径策略与工作区同步

- [x] 3.1 实现默认同步目录 `.` 的创建/映射与宿主路径解析，并允许显式 `/tmp` 路径
- [x] 3.2 实现敏感宿主目录限制和上级目录逃逸限制
- [x] 3.3 通过 JSON-RPC 文件原语完成沙箱内文件变更到宿主可见的同步闭环

## 4. Worker 集成与验证

- [x] 4.1 让 `AgentWorker` 按 capability 配置初始化并暴露真实 sandbox 后端
- [x] 4.2 为 capability 配置、Bubblewrap 不可用、Docker 未实现和路径限制补充单元测试
- [x] 4.3 为 JSON-RPC 工作区文件同步补充验收测试，并在完成后更新任务状态
