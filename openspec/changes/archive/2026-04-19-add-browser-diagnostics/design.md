## Context

当前 `browser` toolset 已经跑通了 `Rust tool -> Node Playwright sidecar -> Chrome` 的最小链路，模型可以用 `snapshot/ref/act` 完成基本网页操作，也可以在动态页面上退回到 `click_match / type_match`。但当前观测面仍然过窄：当页面是“看起来能点、实际点不动”时，线程里只有 snapshot、screenshot 和动作返回值，缺少 console、页面脚本错误、请求失败等诊断事实。

这次变更要在不改变现有模块边界的前提下，为当前 browser 会话补一层可查询的调试观测能力。约束如下：

- 继续复用现有 `src/agent/tool/browser/` 分层，不新增顶层浏览器组件。
- 继续以 Node Playwright sidecar 作为浏览器执行层，不切换协议栈。
- 诊断能力要和线程级 session 对齐，关闭 session 后一并释放。
- 当前 change 只定义诊断观测，不顺带引入 tabs、trace、HAR、response body 等更大范围能力。

## Goals / Non-Goals

**Goals:**

- 为每个 browser session 持续采集 console、页面错误和网络请求摘要。
- 为 `browser` toolset 提供只读诊断工具，让模型或 helper 能查询最近的诊断记录。
- 在保留 browser artifacts 的场景下，把诊断记录落到 session 目录，便于复现与排障。
- 保持工具接口稳定，避免把 Playwright 事件细节直接泄露到 Rust 外层。

**Non-Goals:**

- 本次不实现 trace、HAR、response body、下载明细、上传回放或完整 network inspector。
- 本次不引入新的 thread 持久化模型；诊断记录只活在 live browser session 与可选本地产物里。
- 本次不引入多 tab 显式管理接口，也不要求调用方手动选择观测目标 tab。
- 本次不修改现有 `snapshot/ref/act` 主交互协议的核心语义。

## Decisions

### 1. 诊断事件在 Node sidecar 内持续采集，Rust 侧只做查询与结果规范化

console、page error、request/response/requestfailed 都是浏览器运行中的事件流，不适合像 `snapshot` 一样通过一次同步命令临时求值。首选方案是在 Node sidecar 里为当前 browser context 和 page 注册监听器，持续把事件规整成结构化记录，并存入当前 session 的有界缓冲区；Rust 侧新增只读协议动作，从 sidecar 取回最近记录。

这样做的原因：

- 事件不会因为页面已经跳走而丢失。
- Playwright 对 console/request 生命周期最了解，记录归一化应尽量靠近事件源。
- Rust 侧继续维持 `protocol -> service -> session -> tool` 的薄封装边界。

Alternative considered:

- Rust 按需触发 sidecar 执行 JS，临时读取浏览器状态。
  Rejected，因为 console/pageerror/request 不是稳定的页面状态快照，而是持续发生的事件，临时拉取会漏掉关键事实。

### 2. 使用有界 ring buffer 保存最近诊断记录，避免 session 无限涨内存

诊断信息天然可能很噪，例如大型站点会产生大量 console log 和 network 请求。本次设计要求 sidecar 为每一类记录维护独立的有界缓冲区，只保留最近 N 条记录；工具查询结果默认按时间倒序返回最近记录，并支持 `limit` 进一步收敛返回量。

这样可以兼顾：

- 模型侧可读性
- 运行时内存上界
- 测试可预测性

Alternative considered:

- 不设上限，完整保留所有事件直到 session 关闭。
  Rejected，因为长会话下内存与输出量不可控，和当前 thread-scoped toolset 的轻量目标不匹配。

### 3. 对外暴露三个只读工具：`browser__console`、`browser__errors`、`browser__requests`

当前 browser toolset 的动作接口已经够多，如果再把诊断信息混进 `snapshot` 或 `navigate` 的默认输出，会让主流程响应过于嘈杂。更合理的方式是新增三个显式工具：

- `browser__console`: 查询最近 console 记录
- `browser__errors`: 查询最近页面错误与请求失败
- `browser__requests`: 查询最近请求摘要，可按失败态收敛

这三个工具仍然是 `browser` toolset 的一部分，沿用当前线程级 session，不新增新的 toolset。

Alternative considered:

- 把诊断记录直接拼到 `browser__snapshot` 返回结果。
  Rejected，因为 snapshot 的首要目标是给模型提供“页面可交互视图”，把大量调试日志混进去会稀释观察信号。

### 4. 诊断记录采用统一的归一化字段，而不是直接透传 Playwright 原始对象

工具输出和 artifacts 文件都应基于稳定字段，而不是直接暴露 Playwright 的 event object。首版统一为面向排障的摘要字段：

- console: `timestamp`、`level`、`text`、`page_url`、可选 `location`
- error: `timestamp`、`kind`、`message`、`page_url`、可选 `request_url`
- request: `timestamp`、`method`、`url`、`resource_type`、`status`、`result`

这样能让 Rust、测试和未来 MCP 化迁移都建立在稳定协议上，而不是 JS 运行时对象上。

Alternative considered:

- 直接把 Playwright event JSON 序列化后透传。
  Rejected，因为字段不稳定、噪声高，而且会把 JS 侧实现细节泄露到协议面。

### 5. 只有在 session 保留 artifacts 时才强制落盘诊断文件

当前 browser session 已经支持“默认清理临时目录 / helper 保留 session 目录”两种模式。诊断记录应该始终存在于 live session 的内存缓冲区中；但只有在 `keep_artifacts=true` 的场景下，系统才要求把记录同步写入 `console.jsonl`、`errors.jsonl`、`requests.jsonl` 这类文件。

这样做的原因：

- 普通线程调用仍然保持轻量，不额外制造长期磁盘垃圾。
- helper、smoke 和故障复盘场景可以拿到完整调试产物。

Alternative considered:

- 无论是否保留 artifacts 都强制写文件。
  Rejected，因为这会增加磁盘写放大，并和当前 browser session 的“默认临时、按需保留”策略相冲突。

## Risks / Trade-offs

- [高流量页面会产生大量请求记录] -> 通过有界缓冲区、默认 `limit` 和 `failed_only` 过滤控制输出规模。
- [多次导航后诊断记录混杂不同页面] -> 记录中必须包含 `page_url` 或 `request_url`，由调用方按上下文判读；本次不额外引入 tab 维度管理。
- [console / request 事件归一化过度会丢细节] -> 首版先保留定位问题所需的最小关键字段，后续若出现真实缺口，再通过独立 change 扩字段而不是一次性暴露全部原始对象。
- [artifact 写盘可能影响频繁事件的吞吐] -> 仅在 `keep_artifacts=true` 时启用写盘，并使用 append-only JSONL 降低复杂度。

## Migration Plan

1. 扩展 browser sidecar 协议，增加诊断查询动作与结构化结果类型。
2. 在 Node sidecar 中为 page/context 注册 console、pageerror、request、response、requestfailed 监听，并维护 session 级有界缓冲区。
3. 在 Rust `service/session/tool` 层补齐 `browser__console`、`browser__errors`、`browser__requests` 工具与结果渲染。
4. 在 helper / mock sidecar / 单元测试中补齐诊断路径覆盖，验证可见工具列表、查询结果与 artifact 文件行为。
5. 如实现后噪声过大，再通过默认 limit 或过滤参数调小输出，而不是回退 capability。

Rollback strategy:

- 移除新增协议动作、tool handler 和 sidecar 监听逻辑即可；由于这次变更不修改现有动作语义，也不影响默认主流程，回滚面可控。

## Open Questions

- `browser__errors` 是否需要把 console 的 `error` 级别消息也纳入统一错误视图，还是只包含 page error 与 request failure；本次先在 spec 中要求最小错误面，具体聚合边界可在实现时根据测试样例再定。
- 请求摘要里是否需要包含响应头、耗时等额外字段；本次先不纳入规范，避免把能力范围扩大成完整 network inspector。
