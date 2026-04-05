## 1. OpenSpec 与预算口径

- [x] 1.1 新增 `thread-context-inspection-command` spec，定义 `/context` 与 `/context role` 的输出行为与只读边界
- [x] 1.2 复用现有 deterministic context estimator，并补齐单条 message 估算所需的公共接口

## 2. 命令实现

- [x] 2.1 扩展 `CommandRegistry` 注册 `context` 命令，支持摘要模式与 `role` 明细模式
- [x] 2.2 为 `/context` 命令补充关键日志、稳定文本输出、preview 截断与非法参数错误提示

## 3. 测试验证

- [x] 3.1 在 `tests/command.rs` 中补充 UT，覆盖摘要查询、逐条 message 查询、空线程、非法参数与只读行为
- [x] 3.2 在 `tests/router.rs` 中补充链路测试，确保 `/context` 命令不会触发 agent dispatch 且可正常回包
