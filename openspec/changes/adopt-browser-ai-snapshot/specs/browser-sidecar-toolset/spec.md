## ADDED Requirements

### Requirement: 系统 SHALL 提供基于 AI snapshot 的浏览器语义原子快照能力
系统 SHALL 在 browser sidecar / session 能力中提供一条独立的语义原子快照路径，用于返回当前页面的 AI snapshot 文本。该能力 MUST 以 `_snapshotForAI` 或等价的 AI snapshot 语义为底层来源，并保留页面 role 层级、可见文本、元素 `ref`、状态属性、属性节点和 iframe 子树，而 SHALL NOT 继续沿用当前仅面向 ARIA 断言的旧语义输出作为默认实现。

#### Scenario: 当前页面可以被采集为 AI snapshot
- **WHEN** browser session manager 为当前线程请求一次语义原子快照
- **THEN** 系统返回的结果中包含当前页面的 AI snapshot 文本
- **THEN** 文本中保留可供后续解析的 role/name/`ref`/状态属性层级信息
- **THEN** iframe 内容会按 AI snapshot 语义递归展开到对应子树中

### Requirement: 系统 SHALL 为 AI snapshot 提供明确的兼容与失败语义
当底层 Playwright 不支持 `_snapshotForAI`、AI snapshot 采集失败或返回不合法文本时，系统 SHALL 返回显式失败，而 SHALL NOT 静默退回到旧 `ariaSnapshot()` 输出并冒充 AI snapshot 结果。

#### Scenario: 底层 AI snapshot 不可用时返回显式错误
- **WHEN** 当前运行环境中的 Playwright 无法提供 `_snapshotForAI` 或等价 AI snapshot 调用
- **THEN** browser 语义快照调用会返回可诊断的显式错误
- **THEN** 错误中会说明是 AI snapshot 采集能力不可用，而不是普通导航或解析失败
