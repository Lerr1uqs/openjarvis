## 1. AI snapshot 语义能力

- [ ] 1.1 在 `scripts/browser_sidecar.mjs` 中新增 AI snapshot 采集适配层，并让现有语义快照路径改用 `_snapshotForAI` 或等价 AI snapshot 能力
- [ ] 1.2 更新 Rust browser 协议、service、session 与测试，使 AI snapshot 结果、失败语义和 iframe 行为可验证

## 2. 结构化解析脚本

- [x] 2.1 新增一个独立脚本，读取 AI snapshot 文本并输出稳定 YAML 结构与概念汇总
- [x] 2.2 为解析脚本补充代表性样例测试，覆盖 role、`text`、`/url`、状态属性、iframe 子树和非法输入

## 3. 验证与收敛

- [ ] 3.1 用真实或构造样本验证 `_snapshotForAI` 的概念命名，并把结果纳入 observation / 调试结论
- [ ] 3.2 评估这条语义原子能力是否需要进一步提升为新的公开 browser tool，并补齐对应规范或回归项
