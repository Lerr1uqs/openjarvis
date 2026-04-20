## Context

当前 memory feature 已经完成了三件基础工作：

- 本地 markdown memory 仓库已经可用，`active` / `passive` 的边界明确。
- `memory_search` 已经是模型主动检索 memory 的标准入口。
- `thread-context-runtime` 已经把“普通请求轮次不得自动注入 active memory 正文或摘要”写成正式约束。

但现在的搜索实现仍然只是轻量级文本命中计数，存在几个明显问题：

- 中文 query 在没有分词和字段权重的情况下，召回稳定性较差。
- 同义表达、改写表达和长文本摘要场景下，纯文本匹配容易漏召回。
- 重复 memory、相似 memory 与时间冲突 memory 缺少统一排序策略。
- 如果直接把 embedding / rerank 硬塞进现有实现而不先明确边界，很容易误回到“自动把召回结果塞进上下文”的旧废案。

这次变更的约束已经足够明确：

- hybrid retrieval 只用于 `memory_search` 工具侧检索，不进入 request-time 自动注入。
- 现有 `memory_search(type?, query, limit?)` 对外契约保持不变，不新增重复接口。
- 文本匹配仍然要可用，industrial search 由配置文件显式开启。
- 中文 sparse recall 可以引入 `jieba-rs`。
- dense embedding 使用 `BAAI/bge-large-zh-v1.5`，rerank 使用 `BAAI/bge-reranker-v2-m3`，推理服务使用 SiliconFlow。

## Goals / Non-Goals

**Goals:**

- 为 memory 搜索新增配置驱动的 `lexical` / `hybrid` 两种模式。
- 在 `hybrid` 模式下实现 `BM25 + dense recall -> RRF -> Cross-Encoder rerank -> MMR -> freshness decay` 的固定检索链路。
- 保持 `memory_search` 的现有工具名、入参与结构化返回语义不变。
- 为中文 sparse recall 提供稳定分词能力，避免继续依赖简单空白切词。
- 为 dense recall 引入本地派生 embedding cache，避免每次搜索都重新为全部 memory 文档请求 embedding。
- 把“自动向上下文底部追加 memory 搜索结果或正文”继续标记为废弃设计，并在 spec 中重复约束。
- 补齐可重复的 memory 检索测试语料，覆盖文本命中、语义命中、去冗余、freshness 和远程失败路径。

**Non-Goals:**

- 本次不新增新的 memory tool，不提供 `memory_hybrid_search` 或其他并行接口。
- 本次不恢复或引入 request-time 自动 memory 注入、自动追加底部摘要、自动读取正文等废弃能力。
- 本次不引入外部向量数据库或远程 memory backend；markdown memory 文档仍然是唯一事实来源。
- 本次不修改 active memory catalog 的线程初始化行为，也不为当前线程提供 catalog 热刷新。
- 本次不试图把整个 memory 仓库做成大规模搜索引擎；重点是先把当前工作区 memory 搜索质量与契约做稳。

## Decisions

### 1. 搜索配置放入 `agent.tool.memory.search`，默认保持 `lexical`

这次变更新增 memory 搜索配置段，建议放在 `agent.tool.memory.search`。原因是：

- 当前 browser 等工具侧可配置能力已经放在 `agent.tool.*` 下，位置一致。
- hybrid retrieval 改变的是 `memory_search` 这一工具行为，而不是线程初始化 catalog 或 feature 开关本身。
- 保持默认 `lexical`，可以保证没有 SiliconFlow 凭据或未显式启用配置的工作区继续按本地文本匹配工作。

建议配置形态如下：

```yaml
agent:
  tool:
    memory:
      search:
        mode: lexical
        hybrid:
          base_url: "https://api.siliconflow.cn/v1"
          api_key_path: "~/.siliconflow.apikey"
          embedding_model: "BAAI/bge-large-zh-v1.5"
          rerank_model: "BAAI/bge-reranker-v2-m3"
          bm25_top_n: 30
          dense_top_n: 30
          rerank_top_n: 20
          rrf_k: 60
          mmr_lambda: 0.7
          freshness_half_life_days: 30
```

Alternative considered:

- 把配置放到顶层 `memory.search`。
  Rejected，因为当前配置树没有独立 top-level memory section，而这次变化直接作用于工具侧检索路径。

- 默认直接切到 `hybrid`。
  Rejected，因为这会把当前无远程凭据的本地场景变成默认失败，不符合“文本匹配始终可用”的要求。

### 2. 保持 `memory_search` 对外 contract 不变，只替换内部搜索运行时

`memory_search` 仍然接收现有的 `query`、`type?` 与 `limit?`，并继续返回结构化 `items`，不直接返回正文。这样可以避免：

- 出现两个并列的 memory 搜索接口，违背“同一能力不允许出两个接口”的项目约束。
- 让模型必须重新学习新的工具名或重复 tool schema。
- 让 search 再次退化成“隐式读取正文”的旁路。

内部实现则按配置分流：

- `mode=lexical` 时，继续执行本地文本搜索。
- `mode=hybrid` 时，进入新的混合检索 pipeline。

Alternative considered:

- 新增 `memory_search_hybrid`。
  Rejected，因为会把同一能力拆成两个公开接口，增加模型选择歧义。

### 3. sparse recall 使用 `jieba-rs` + BM25，dense recall 使用 SiliconFlow embedding

hybrid 模式的第一层召回拆成两路：

- sparse recall: 对 query 与 memory 文档执行一致的中文分词，使用 BM25 产生 `bm25_top_n` 候选。
- dense recall: 对 query 生成 embedding，并与本地缓存的文档 embedding 做相似度召回，产生 `dense_top_n` 候选。

文档的 sparse / dense 表示建议使用以下字段：

- sparse: `title + keywords + path + content`
- dense: `title + keywords + content`

这样设计的原因是：

- `path` 对文本精确命中有价值，但对语义 embedding 噪音更大。
- `keywords` 对 active memory 的 sparse / dense 都有帮助。
- `content` 仍然是正文相关性的主体。

Alternative considered:

- 继续复用当前简单 `contains` 逻辑作为 sparse recall。
  Rejected，因为这无法支撑中文 recall 的质量目标，也和“industrial search”目标不匹配。

- 在 hybrid 模式里直接用 dense recall，省掉 BM25。
  Rejected，因为 memory 中的显式术语、路径名和缩写仍然非常适合 sparse recall，单路 dense 会丢掉这类精确召回优势。

### 4. dense 文档 embedding 使用本地派生 cache，而不是每次搜索重新远程计算

为避免每次 `memory_search` 都对全部文档发起 embedding 请求，系统引入本地派生 cache。这个 cache 不是新的事实来源，只是 markdown memory 的派生数据。建议做法：

- 按 `memory_type + path + content fingerprint + embedding_model` 生成稳定 cache key。
- 文档 fingerprint 变化时，仅重建对应文档的 dense embedding。
- query embedding 始终实时请求，不做长期持久化。
- cache 存储在 `.openjarvis/memory` 下的隐藏派生目录，例如 `.openjarvis/memory/.retrieval/`。

这样可以在不引入外部向量数据库的前提下，把 dense recall 成本控制在“增量文档更新 + 每次 query 一次 embedding 请求”。

Alternative considered:

- 每次搜索都重新为所有文档计算 embedding。
  Rejected，因为成本、延迟和 SiliconFlow 请求量都不可接受。

- 直接引入外部向量数据库。
  Rejected，因为当前 memory 的事实来源仍然应该是本地 markdown，外部 DB 会显著扩大系统边界。

### 5. hybrid 排序链路固定为 `RRF -> rerank -> MMR -> freshness decay`

在混合召回后，排序过程固定如下：

1. 使用 RRF 融合 BM25 与 dense 的候选排名。
2. 取融合后的前 `rerank_top_n` 调用 SiliconFlow rerank。
3. 用 rerank relevance 作为 MMR 的 relevance 输入，结合文档 embedding 做最终去冗余。
4. 对 MMR 结果施加 freshness decay，基于 `updated_at` 做时间衰减。
5. 截断为最终 `limit` / top-k。

原因：

- RRF 对不同来源分数尺度不敏感，适合融合 sparse / dense 两路召回。
- rerank 放在召回层之后，可以把远程算力集中在更小候选集上。
- MMR 放在 rerank 之后，能更好地在高相关候选之间做多样性控制。
- freshness 放在最后，避免新文档仅凭时间优势压倒明显更相关的旧文档。

Alternative considered:

- 用加权分数直接混合 BM25 与 cosine score。
  Rejected，因为两路分数量纲不稳定，调参脆弱，RRF 更稳。

- 在 rerank 前做 MMR。
  Rejected，因为去冗余应建立在更可靠的 relevance 基础上，而不是原始召回分数。

### 6. hybrid 失败时 fail-fast，不静默回退到 lexical

当配置显式启用 `mode=hybrid` 时，若发生以下问题，`memory_search` 应直接报错并记录调试日志，而不是静默降级到 lexical：

- SiliconFlow API key 缺失或不可读
- embedding / rerank 请求失败
- 返回 payload 非法
- 必需模型配置为空

这样更符合当前项目的调试取向：

- 配置打开后，调用方应得到确定行为，而不是表面成功但实际退回旧模式。
- 静默回退会让检索质量问题更难定位。

Alternative considered:

- hybrid 失败时自动回退 lexical。
  Rejected，因为这会掩盖配置错误与远程依赖故障，降低系统可观测性。

### 7. hybrid retrieval 明确只属于工具侧检索，不改变 thread runtime 的 memory 边界

虽然这次引入了 embedding、rerank 和派生 cache，但 memory 的运行时边界保持不变：

- thread 初始化阶段仍然只注入 active memory catalog。
- 普通请求轮次仍然不得自动追加 memory 搜索结果、摘要或正文。
- 模型只有在显式调用 `memory_search` / `memory_get` 时才获得 memory 详情。

`thread-context-runtime` 已经有现成 requirement，本次 `memory-feature` spec 会再次重复该约束，防止实现阶段误把 hybrid retrieval 接到 request-time prompt 组装链路上。

Alternative considered:

- 把 hybrid recall 结果作为 request-time 底部 context 自动注入。
  Rejected，因为这是已经废弃的旧方案，会直接破坏当前渐进式披露边界。

## Risks / Trade-offs

- [Risk] SiliconFlow 依赖引入远程失败面和额外延迟 -> Mitigation: 默认保持 `lexical`；只有显式开启 `hybrid` 才依赖远程服务，并在关键阶段补齐调试日志。
- [Risk] 本地 embedding cache 可能与 markdown 文档漂移 -> Mitigation: 用 `updated_at + content fingerprint + model id` 驱动失效，只把 cache 视为派生数据。
- [Risk] jieba 分词与用户预期词界不完全一致 -> Mitigation: 保留 dense recall 并用 RRF 融合，避免让 sparse 成为单点质量瓶颈。
- [Risk] freshness decay 过强会压低高相关旧文档 -> Mitigation: 通过半衰期配置做轻量衰减，并放在 MMR 之后。
- [Risk] 当前 `model/memory.md` 仍把 embedding / 向量索引排除在组件边界外 -> Mitigation: 本次只在 OpenSpec 中先定变更；实现阶段若落地，需要在用户允许下同步组件文档。

## Migration Plan

1. 新增 memory 搜索配置结构，默认值保持 `lexical`，确保现有工作区零配置可继续运行。
2. 在 memory 模块内部引入分词、BM25、dense cache 与 SiliconFlow client，但不改 `memory_search` 的公开工具名和返回结构。
3. 将 `memory_search` 的内部流程替换为“按 mode 分流”，并补齐关键日志、错误路径与 cache 失效逻辑。
4. 增加 fixture corpus 与 UT，覆盖 lexical 基线、hybrid 召回、rerank、MMR、freshness 和远程失败。
5. 在实现完成后，再由用户确认是否同步 `model/memory.md` 等组件文档，避免未经允许修改现有建模文档。

Rollback strategy:

- 若 hybrid 路径出现风险，可以把默认配置保持 `lexical`，并通过关闭 `mode=hybrid` 立即回退到现有文本匹配行为；因为 `memory_search` 对外 contract 不变，回滚影响面主要在内部实现和配置。

## Open Questions

- 当前无阻塞性开放问题；实现阶段只需要把 cache 路径命名和日志字段命名收敛为现有代码风格即可。
