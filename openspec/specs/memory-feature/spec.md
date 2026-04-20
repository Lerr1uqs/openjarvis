# memory-feature Specification

## Purpose
Define the workspace memory retrieval capability so `memory_search` can run in configurable `lexical` or `hybrid` mode while preserving gradual disclosure, structured search results, and the prohibition on request-time automatic memory injection.
## Requirements
### Requirement: 系统 SHALL 为 `memory_search` 提供配置驱动的搜索模式
系统 SHALL 为 `memory_search` 提供配置驱动的搜索模式，至少支持 `lexical` 与 `hybrid` 两种模式。未显式开启 `hybrid` 时，系统 SHALL 保持本地文本匹配可用，而 SHALL NOT 强依赖远程 embedding 或 rerank 服务；显式开启 `hybrid` 时，系统 SHALL 允许通过配置声明 `base_url`、`api_key_path`、`embedding_model`、`rerank_model` 与召回/排序参数。若这些字段未显式覆盖，系统 SHALL 默认使用 SiliconFlow `https://api.siliconflow.cn/v1`、`BAAI/bge-large-zh-v1.5` 与 `BAAI/bge-reranker-v2-m3`。

#### Scenario: 未开启 hybrid 时仍可执行本地文本匹配
- **WHEN** 工作区没有为 memory search 显式开启 `hybrid` 模式
- **THEN** `memory_search` 仍然可以仅依赖本地 memory 文档执行文本匹配
- **THEN** 调用方不需要提供 SiliconFlow 凭据也能得到结构化候选结果

#### Scenario: 开启 hybrid 时未覆盖模型参数会使用默认 SiliconFlow 配置
- **WHEN** 配置文件把 memory search 模式切到 `hybrid`
- **AND** 配置中没有显式覆盖 `base_url`、`embedding_model` 或 `rerank_model`
- **THEN** 系统会把 `https://api.siliconflow.cn/v1` 作为默认推理地址
- **THEN** 系统会把 `BAAI/bge-large-zh-v1.5` 与 `BAAI/bge-reranker-v2-m3` 作为默认模型

### Requirement: `hybrid` 模式 SHALL 使用固定的多阶段检索链路
当 `memory_search` 运行在 `hybrid` 模式时，系统 SHALL 按固定顺序执行 `BM25 + dense` 多路召回、`RRF` 融合、Cross-Encoder rerank、`MMR` 去冗余和 freshness decay。该链路 SHALL 作用于当前 `type` 过滤后的 memory 文档集合，并 SHALL 同时处理中文文本命中、语义相近表达、近重复候选与时间新鲜度差异。

#### Scenario: 语义相关但词面不同的 memory 能进入 hybrid top-k
- **WHEN** 某条 memory 文档与 query 在语义上高度相关，但正文中不包含完全相同的词面表达
- **AND** 当前 memory search 模式为 `hybrid`
- **THEN** 该文档仍然可以通过 dense recall、RRF 与 rerank 进入最终 top-k

#### Scenario: 近重复候选会在最终结果中被去冗余
- **WHEN** 两条 memory 文档都被 hybrid 召回，且正文大体重复、主题高度相同
- **THEN** 最终 top-k 不会机械地把多个近重复候选都排在前列
- **THEN** 系统会在最终结果中保留更有代表性的候选并压低重复项

#### Scenario: 新旧冲突文档会在高相关前提下考虑 freshness
- **WHEN** 两条 memory 文档主题接近、相关性接近，但 `updated_at` 明显一新一旧
- **THEN** freshness decay 会在最终排序阶段对较新的文档给予更高优先级
- **THEN** 该时间偏置不会跳过前面的相关性与去冗余阶段直接生效

### Requirement: `memory_search` SHALL 保持渐进式披露与现有工具契约
无论运行在 `lexical` 还是 `hybrid` 模式，`memory_search` SHALL 继续复用现有工具名、现有入参与结构化返回语义。系统 SHALL 只返回候选摘要、路径、标题、关键词和时间等结构化信息，而 SHALL NOT 直接返回 memory 正文。hybrid retrieval SHALL 只属于模型显式调用 `memory_search` 的工具侧检索能力，而 SHALL NOT 恢复 request-time 自动向上下文追加 memory 搜索结果、摘要或正文的旧设计。

#### Scenario: memory_search 结果仍然不包含正文
- **WHEN** 模型调用 `memory_search`
- **THEN** 返回结果中包含的是结构化候选项，而不是完整 memory 正文
- **THEN** 模型若需要正文仍然必须继续调用 `memory_get`

#### Scenario: 普通请求轮次不会因为 hybrid search 存在而自动注入 memory
- **WHEN** 某一轮用户输入命中了 active memory keyword 或与某些 memory 文档语义相关
- **AND** 模型在该轮中没有显式调用 `memory_search` 或 `memory_get`
- **THEN** 系统不会自动把 memory 搜索结果、摘要或正文附加到该轮 LLM 请求上下文中

### Requirement: `hybrid` 模式在远程依赖异常时 SHALL 显式失败
当配置显式启用 `hybrid` 模式时，系统 SHALL 将 embedding / rerank 的远程依赖视为该模式的必需组件。若 `api_key_path` 缺失、凭据不可读、请求返回非成功状态、payload 非法或必需模型配置为空，`memory_search` SHALL 显式返回错误，而 SHALL NOT 静默回退到 `lexical` 模式。

#### Scenario: hybrid 配置缺少可读凭据时搜索直接失败
- **WHEN** memory search 模式被显式设置为 `hybrid`
- **AND** `api_key_path` 指向的凭据文件不存在或不可读
- **THEN** `memory_search` 调用会直接返回错误
- **THEN** 系统不会在同一次调用里静默降级为 `lexical`

#### Scenario: 远程 provider 返回错误时不会静默降级
- **WHEN** memory search 模式为 `hybrid`
- **AND** embedding 或 rerank 的远程 provider 请求返回非成功状态或非法响应体
- **THEN** `memory_search` 调用会把该错误暴露给调用方
- **THEN** 系统不会表面返回成功但暗中改走纯文本匹配
