## Context

当前 browser sidecar 已经提供一套面向动作续接的扁平 `snapshot/ref/act` 协议，同时在 service / session 层保留了一条 `aria_snapshot` 语义观测路径。但这条语义路径目前仍然基于 `locator('body').ariaSnapshot()`，输出更偏断言和辅助功能视角，缺少对 agent 友好的 `ref`、iframe 递归展开和状态位表达；而 Playwright 对外已经把这类能力表述为 `page.ariaSnapshot({ mode: "ai" })` / `locator.ariaSnapshot({ mode: "ai" })`，说明 AI snapshot 已经成为更合理的页面语义载体。

同时，AI snapshot 虽然本质上是 YAML 风格文本，但 role、name、属性和子树都编码在 plain scalar 里，直接拿文本做后处理会很脆弱，因此需要一个独立 helper 先把它规整成稳定 YAML 结构。

## Goals / Non-Goals

**Goals:**
- 用 AI snapshot 语义替换当前 `aria_snapshot` 的底层采集路径，形成更适合 agent 消费的语义原子快照。
- 明确 AI snapshot 中可依赖的概念命名，包括 role、`text`、`/url`、`ref`、状态属性和 iframe 子树。
- 提供一个独立脚本，把 AI snapshot 文本解析为稳定 YAML 结构，并输出概念汇总，方便后续规则处理。
- 保持现有动作续接链路可用，不让 click/type/ref 因语义快照升级而立即回归。

**Non-Goals:**
- 本次不直接废弃现有面向动作的扁平 `browser__snapshot` 输出。
- 本次不引入新的模型组件文档或新的长期运行服务。
- 本次不为私有或历史 AI snapshot 入口提供兼容层。

## Decisions

### 1. AI snapshot 作为独立的语义原子能力，而不是直接覆盖动作型扁平 snapshot

当前扁平 `browser__snapshot` 的价值在于给后续 `click_ref` / `type_ref` 提供紧凑元素表，而 AI snapshot 的价值在于提供层级语义树。这两者解决的问题不同，直接把一个替掉另一个会让 ref 续接链路和模型使用习惯一起变化，回归面过大。

因此本次设计选择是：
- 用 `ariaSnapshot({ mode: "ai" })` 语义替换现有 `aria_snapshot` 采集路径
- 把这条路径定义为 browser 的语义原子 snapshot 能力
- 暂不改变现有扁平 snapshot 的默认动作续接语义

备选方案：
- 直接让 `browser__snapshot` 改成返回 AI snapshot。Rejected，因为这会同时改变模型观察形式、ref 解析方式和历史 prompt 习惯，风险过高。

### 2. 采集层只使用公开 `mode="ai"` 语义

用户已经确认目标接口应为 `page.ariaSnapshot({ mode: "ai" })` / `locator.ariaSnapshot({ mode: "ai" })`。因此本次设计直接以公开 `mode="ai"` 语义作为唯一采集入口，而不是再为历史或私有实现保留兼容分支。

这样做的原因：
- 规范层直接贴齐 Playwright 的公开 API 语义，后续沟通成本更低
- 可以避免把私有入口继续固化进项目约束
- Rust 协议与 parser 脚本都围绕“AI snapshot 文本契约”而不是某个历史实现名

备选方案：
- 保留私有或历史入口作为 fallback。Rejected，因为用户已经明确不再支持这条路径。

### 3. 解析脚本输出“稳定 AST 风格 YAML”，而不是原样转存文本

AI snapshot 文本虽然是 YAML 风格，但单行 key 里混合了 role、name、属性和值，后续规则处理如果继续直接 string match，会很难维护。解析脚本输出将收敛为：
- 节点列表 / 根节点树
- `role`、`name`、`ref`
- 泛化后的 `attributes`
- `props`
- `value`
- `children`
- `summary` 中提取出的 roles / attributes / properties 概念集合

备选方案：
- 只做“文本是否合法 YAML”的包装脚本。Rejected，因为这不能解决后续结构化消费问题。

## Risks / Trade-offs

- [当前项目依赖版本如果尚未支持 `mode="ai"`] → 在实现前先对齐 Playwright 版本或调用方式，不静默回退到旧输出。
- [AI snapshot 输出格式存在版本漂移] → parser 采用宽松的属性解析和保守的 YAML AST，尽量保留未知属性而不是硬编码丢弃。
- [iframe 递归或属性节点解析不完整] → 在脚本测试里加入 iframe、`/url`、`text`、状态位等代表性样例。
- [两类 snapshot 并存导致调用方困惑] → 在 spec 中明确“扁平 snapshot 用于动作续接，AI snapshot 用于语义观察与后处理”。

## Migration Plan

1. 在 sidecar 中增加基于 `ariaSnapshot({ mode: "ai" })` 的 AI snapshot 采集实现，并让现有 `aria_snapshot` 路径改走该实现。
2. 在 Rust 协议 / service / session 层把结果类型与命名更新为 AI snapshot 语义。
3. 增加独立解析脚本，先用于 observation、调试与后处理验证。
4. 完成测试与 observation 验证后，再决定是否把这条语义能力提升到新的公开 browser tool。

## Open Questions

- 最终是否要把这条语义原子能力直接提升为新的模型可见 browser tool，还是先停留在 session / helper 层。
- 当前项目依赖版本是否已经完整支持 `ariaSnapshot({ mode: "ai" })`，还是需要先升级 Playwright 版本。
