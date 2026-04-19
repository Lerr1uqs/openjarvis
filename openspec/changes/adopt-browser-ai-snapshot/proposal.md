## Why

当前浏览器侧已经同时存在两种页面观测思路：一类是面向动作续接的 `browser__snapshot` 扁平元素列表，另一类是面向语义结构观察的 `aria_snapshot`。现有默认 `ariaSnapshot()` 输出更偏断言/可访问性视角，对 agent 后续做层级理解、iframe 递归处理和结构化后处理都不够顺手；而 Playwright 对外公开的 `ariaSnapshot({ mode: 'ai' })` 产出的 AI snapshot 在层级、`ref`、状态位和 iframe 展开上更贴近模型消费路径，因此现在适合把它整理成 browser 侧新的原子 snapshot 能力。

## What Changes

- 将 browser sidecar 当前使用的 `ariaSnapshot()` 语义升级为基于 `ariaSnapshot({ mode: 'ai' })` 的 AI snapshot 语义，并把它定义为对外可用的原子页面语义快照能力。
- 明确 AI snapshot 中的核心概念命名，包括 role、name、text、`ref`、状态属性、属性节点和 iframe 子树等，统一为可解析的契约。
- 新增一个独立脚本，把 AI snapshot 文本解析成结构化 YAML，供后续规则处理、数据抽取或 diff 使用。
- 为 `ariaSnapshot({ mode: 'ai' })` 调用失败或输出异常的场景补充明确失败语义，避免 browser 观察链路静默退化。

## Capabilities

### New Capabilities
- `browser-ai-snapshot-parser`: 提供一个独立 helper，把 AI snapshot 文本解析成稳定的 YAML 结构，便于后续处理和分析。

### Modified Capabilities
- `browser-sidecar-toolset`: browser 语义 snapshot 的底层来源与对外契约从现有 ARIA snapshot 语义升级为 AI snapshot 风格的原子能力。

## Impact

- 受影响代码主要在 `scripts/browser_sidecar.mjs`、`src/agent/tool/browser/*`、`tests/agent/tool/browser/*` 与新增的 `scripts/*` helper。
- 需要依赖 Playwright 的 `ariaSnapshot({ mode: 'ai' })` 能力，并补充对其输出格式的解析与测试。
- browser 相关 observation / 调试脚本会新增一种可结构化消费的 AI snapshot 产物，供后续 agent 规则复用。
