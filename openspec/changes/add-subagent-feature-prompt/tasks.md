## 1. Feature 建模与初始化接线

- [ ] 1.1 为主线程新增 `Feature::Subagent` 或等价 feature 开关，并明确 child thread 不继承该 feature
- [ ] 1.2 在线程初始化或重初始化时新增 subagent feature prompt 构造入口，按稳定顺序写入主线程 `System` 前缀
- [ ] 1.3 让 subagent feature prompt 基于当前可用 subagent catalog 生成，至少覆盖数量、profile 摘要和使用时机

## 2. 能力暴露与可见性

- [ ] 2.1 让 `spawn_subagent`、`send_subagent`、`close_subagent`、`list_subagent` 的主线程可见性由 subagent feature 控制
- [ ] 2.2 明确 child thread 不暴露父线程 subagent 管理 prompt 与工具
- [ ] 2.3 补齐“feature 关闭时主线程看不到 subagent prompt/工具”的约束

## 3. 测试与验证

- [ ] 3.1 补齐主线程初始化测试，覆盖 subagent feature prompt 注入内容包含可用 subagent 数量与使用时机
- [ ] 3.2 补齐 child thread 初始化测试，覆盖 child thread 不继承父线程 subagent feature prompt
- [ ] 3.3 补齐 tool visibility 测试，覆盖 feature 开/关对 subagent 管理工具可见性的影响
