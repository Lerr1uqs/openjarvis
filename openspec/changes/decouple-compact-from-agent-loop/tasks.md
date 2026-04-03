## 1. Compactor 组件提炼

- [ ] 1.1 在 `compact/` 模块中新增可独立实例化的 `ContextCompactor` component，并定义清晰的输入输出 contract
- [ ] 1.2 将 compact provider 调用、strategy 应用和 compact outcome 生成逻辑收口到该 component
- [ ] 1.3 保持现有 compact summary、replacement turn 和 `CompactionOutcome` 语义不变

## 2. AgentLoop 接线迁移

- [ ] 2.1 调整 AgentLoop，使 runtime compact 路径通过显式 compactor component 调用执行
- [ ] 2.2 调整模型触发 `compact` tool 的路径，使其与 runtime compact 共用同一 compactor execution contract
- [ ] 2.3 移除 AgentLoop 对长期 `CompactManager` 成员的直接依赖

## 3. 验证

- [ ] 3.1 为 standalone compactor 增加 UT，覆盖空线程、正常 compact 和 strategy/provider 注入路径
- [ ] 3.2 更新 AgentLoop 相关测试，覆盖“通过外部 compactor component 串联调用”后的行为一致性
