支持主动式记忆和被动式记忆（Active Memory 和 Passive Memory），并以工作区下的本地 markdown memory 目录作为唯一事实来源。

1. 被动式记忆
   被动式记忆写入 `./.openjarvis/memory/passive/`，不会在 thread 初始化时自动披露，只能通过 `memory_list`、`memory_search`、`memory_get` 按需访问。

2. 主动式记忆
   主动式记忆写入 `./.openjarvis/memory/active/`，要求带有稳定关键词。
   在线程初始化或重初始化时，系统只把 `keyword -> path` catalog 作为渐进式披露注入 system prompt，不自动注入正文。

3. 搜索与读取
   `memory_search` 是统一的搜索入口，默认提供本地 `lexical` 检索，也可以通过配置启用 `hybrid` 检索。
   `hybrid` 模式在工具侧执行 `BM25 + dense recall -> RRF -> rerank -> MMR -> freshness decay`，并使用本地派生 embedding cache 与远程 rerank/embedding 服务提升召回和排序质量。
   无论使用哪种模式，`memory_search` 都只返回结构化候选，不直接返回正文；需要正文时继续使用 `memory_get`。

4. 写入与边界
   `memory_write` 负责把新的 active/passive memory 写回本地仓库，但不会回写当前线程已经持久化的旧 catalog。
   普通请求轮次不会因为关键词命中或语义相关而自动把 memory 搜索结果、摘要或正文注入到 LLM 上下文。
   当显式启用 `hybrid` 且远程依赖异常时，`memory_search` 会显式失败，不静默回退到 `lexical`。
