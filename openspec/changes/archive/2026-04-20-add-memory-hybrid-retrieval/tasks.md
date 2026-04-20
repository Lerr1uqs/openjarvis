## 1. 配置与检索运行时骨架

- [x] 1.1 扩展 `src/config.rs`，新增 `agent.tool.memory.search` 配置结构，覆盖 `mode=lexical|hybrid`、SiliconFlow 默认地址、默认模型和召回/排序参数解析
- [x] 1.2 在 memory 模块中新增检索运行时与 SiliconFlow client 边界，统一处理 `api_key_path` 读取、请求组装、响应解析和关键调试日志

## 2. Hybrid Retrieval Pipeline

- [x] 2.1 在 `src/agent/memory/**` 中引入中文 sparse recall，使用 `jieba-rs` 完成 query / document 分词并实现 BM25 候选召回
- [x] 2.2 为 memory 文档实现本地 dense embedding 派生 cache 与失效规则，按 `type + path + fingerprint + model` 增量刷新
- [x] 2.3 实现 `dense recall -> RRF -> rerank -> MMR -> freshness decay` 的完整排序链路，并支持按配置截断 `bm25_top_n`、`dense_top_n`、`rerank_top_n` 与最终 top-k

## 3. Memory Toolset 接线与边界约束

- [x] 3.1 在不改变 `memory_search` 工具名、参数和结构化返回格式的前提下，将 repository/search 逻辑切换为按配置选择 `lexical` 或 `hybrid`
- [x] 3.2 确保 `hybrid` 模式在凭据缺失、远程返回错误或响应非法时显式失败，不静默回退到 `lexical`
- [x] 3.3 补齐回归约束，确保 hybrid retrieval 只用于工具侧检索，不会恢复 request-time 自动注入 memory 搜索结果、摘要或正文

## 4. 测试语料与自动化验证

- [x] 4.1 在 `tests/agent/memory/` 下新增配置与检索单测，覆盖默认 `lexical`、显式 `hybrid`、默认模型解析和远程失败路径
- [x] 4.2 设计并落地一组可重复的 memory fixture corpus，覆盖文本精确命中、语义命中、近重复去冗余、新旧文档 freshness、噪声文档与 `type` 过滤
- [x] 4.3 补齐 toolset / thread runtime 回归测试，确认 `memory_search` 仍然只返回结构化候选，且普通请求轮次不会因为 hybrid search 存在而自动注入 memory

## 5. 文档与实现收口

- [x] 5.1 在用户确认后同步 `model/memory.md` 与相关能力文档边界，明确 embedding / rerank / 派生检索 cache 已成为 memory 搜索实现的一部分，但 request-time 自动注入仍然是废弃方案
