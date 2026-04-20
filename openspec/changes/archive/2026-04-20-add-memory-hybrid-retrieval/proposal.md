## Why

当前 `memory_search` 仍然停留在简单的纯文本命中计数，面对中文长文本、近义表达、重复记忆和时序冲突时，召回质量与排序稳定性都不足。既然 memory 已经成为模型主动检索的唯一入口，现在需要把搜索能力升级为“默认可用的文本匹配 + 配置驱动的 hybrid retrieval”，同时继续明确禁止回到旧版“自动向上下文底部注入 memory 正文”的废弃设计。

## What Changes

- 为 memory 搜索新增配置驱动的检索模式，至少区分默认文本匹配模式与 hybrid retrieval 模式。
- 在 hybrid retrieval 模式下，将 memory 搜索链路升级为 `BM25 + dense recall -> RRF -> Cross-Encoder rerank -> MMR -> freshness decay`。
- 明确 dense embedding 使用 `BAAI/bge-large-zh-v1.5`，rerank 使用 `BAAI/bge-reranker-v2-m3`，远程推理服务使用 SiliconFlow `https://api.siliconflow.cn/v1`。
- 为中文 sparse recall 引入稳定分词方案，允许实现侧使用 `jieba-rs` 提升 BM25 质量。
- 维持 `memory_search` 的结构化返回语义，只返回候选摘要与排序结果，不直接返回正文。
- 明确 hybrid retrieval 只作用于模型显式调用 `memory_search` 的工具侧检索，不得恢复“请求期自动注入 memory 搜索结果或正文”的旧方案。
- 为 memory 检索补齐可重复的测试语料与回归测试，覆盖文本命中、语义命中、重复去冗余与时间新鲜度排序。

## Capabilities

### New Capabilities
- `memory-feature`: 定义 memory 搜索模式、hybrid retrieval 排序链路、配置契约，以及“禁止自动注入上下文”的检索边界。

### Modified Capabilities

## Impact

- Affected code: `src/agent/memory/**`、`src/config.rs`、可能新增 memory 检索子模块，以及对应 `tests/agent/memory/**`。
- Affected dependencies: 需要新增中文分词依赖 `jieba-rs`，并接入 SiliconFlow embedding / rerank HTTP 调用。
- Affected runtime data: 可能新增 memory 派生检索缓存或索引数据，但 markdown memory 文档仍然是唯一事实来源。
- API impact: 对外继续复用现有 `memory_search` 工具，不新增重复能力接口；新增的是配置项与内部排序行为。
- Spec impact: 新增 `memory-feature` capability，显式约束 hybrid retrieval 的配置和运行边界，并再次强调禁止恢复 request-time 自动注入 memory 正文。
