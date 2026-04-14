## Context

当前 browser sidecar toolset 的首版约束非常明确：每个线程会话都会创建新的临时 `user-data-dir`，不会复用系统默认 Chrome Profile，也不会保留长期账号态。这让最小 observe-act 闭环比较干净，但对真实网站任务仍然有两个明显短板：

- 需要登录的网站每次都要重新登录，agent 无法复用已经验证过的 cookies。
- 用户已经手动启动并登录好的 Chromium 实例无法被当前 browser toolset 复用。

这次用户又明确补充了两个接口层面的要求：

- cookies 不再作为两个常驻 browser 工具显式暴露，而是通过配置文件 flag 控制 open 自动注入、close 自动导出，并保留一个显式 Command / helper 做手动导出。
- attach 和 launch 不再拆成独立动作，而是统一收敛到 `browser__open` 的参数里。

因此，这次变更既是“能力新增”，也是对 browser session 生命周期入口的一次收敛。

## Goals / Non-Goals

**Goals:**

- 为 browser runtime 提供配置文件驱动的 cookies 状态复用能力，支持显式状态文件路径、open 自动注入和 close 自动导出。
- 提供一个显式 Command / helper，让用户可以把当前 session cookies 手动导出落盘。
- 为 browser toolset 提供统一的 `browser__open(mode=launch|attach, ...)` 会话入口。
- 保持当前线程级 session 模型不变，不引入新的顶层浏览器子系统。

**Non-Goals:**

- 本次不改成长期复用整个 `user-data-dir` 或系统默认 Chrome Profile。
- 本次不把 localStorage、sessionStorage、IndexedDB 等完整 storage state 一起纳入规范；首版只保证 cookies。
- 本次不实现“自动发现本机正在运行的 Chrome 并附着”；attach 必须依赖显式 CDP endpoint。
- 本次不引入多浏览器共享写入同一个 cookies 文件的并发协调机制。
- 本次不把 cookies 导出/加载做成新的常驻 browser tool。

## Decisions

### 1. 用户状态复用首版只做 cookies 状态文件，不做 profile 目录复用

要解决“避免重复登录”，边界最清晰的方案仍然不是复用整个浏览器 profile，而是把当前会话 cookies 写入显式状态文件，再在后续 launch 会话中重新加载。这样可以继续维持首版关于独立 `user-data-dir` 的安全约束，同时给浏览器状态一个可审计、可复制、可清理的载体。

这次设计仍然把 cookies 作为显式文件来管理，而不是把“复用登录态”等同于“复用整个 profile 目录”。

Alternative considered:

- 直接长期保留并复用某个固定 `user-data-dir`。
  Rejected，因为这会把 profile 膨胀成未受控的状态容器，和当前“独立临时目录、不碰默认 Profile”的边界冲突。

### 2. cookies 复用通过配置驱动的 open/close 生命周期实现，并保留手动导出 Command / helper

用户已经明确这次不要常驻的 `browser__export_cookies` / `browser__load_cookies`。因此 cookies 复用应该收敛到 browser 生命周期上：

- 配置文件提供显式 cookies 状态文件路径
- 配置文件提供 open 自动注入 flag
- 配置文件提供 close 自动导出 flag

当线程通过 launch 路径建立 browser session 时，系统按配置在 open 阶段自动注入 cookies；当线程执行 `browser__close` 时，系统按配置自动导出 cookies。除此之外，再提供一个显式 Command / helper，允许用户在任意需要的时点把当前 session cookies 手动导出到指定路径。

这里有两个额外边界需要明确：

- 自动 cookies 注入只适用于 launch 会话和与其等价的默认 lazy launch 初始化，不适用于 attach 到外部浏览器的会话。
- 当自动注入开启但状态文件尚不存在时，首次 launch 不应该失败；这是正常的“第一次登录前”场景。

Alternative considered:

- 继续暴露 `browser__export_cookies` / `browser__load_cookies` 两个 browser 工具。
  Rejected，因为 cookies 持久化更像 browser runtime policy 和显式用户命令，而不是每轮 agent 都需要看到的常驻浏览器动作工具。

### 3. attach 与 launch 统一收敛到 `browser__open`

browser session 的来源本质上只有两种：

- launch：系统自己启动 Chromium
- attach：系统连接到外部已存在的 Chromium

把这两条路径拆成 `browser__open + browser__attach` 两个入口，会让 session 生命周期和错误语义分散。统一做成 `browser__open(mode=launch|attach, ...)` 后，调用方只需要理解“我现在在请求哪一种会话来源”，而不是再额外记忆一个 attach 动作。

其中 attach 仍然必须依赖显式的 `cdp_endpoint`，不能自动猜测，更不能隐式附着到默认浏览器。

Alternative considered:

- 保留独立 `browser__attach` 动作。
  Rejected，因为这会把同一种 session 打开语义拆成两个入口，导致工具面、helper 路径和错误处理都更难收敛。

### 4. 现有 lazy session 初始化继续保留，但语义上等价于 `browser__open(mode=launch)`

当前 browser toolset 已经依赖“首次动作时懒建 session”的行为。为了不把现有链路全部打断，这次不要求调用方必须先显式执行 `browser__open`。相反，设计要求：

- 如果调用方显式执行 `browser__open`，系统按其参数建立或替换当前 session。
- 如果调用方没有显式 open，而是直接执行 `navigate / snapshot / click / type / screenshot`，系统就走一个与 `browser__open(mode=launch)` 等价的默认 launch 初始化路径。

这样可以保留向后兼容，同时保证 cookies 自动注入、launch / attach 互斥和 close 语义只维护一套事实。

Alternative considered:

- 强制所有浏览器动作前必须显式调用 `browser__open`。
  Rejected，因为这会直接改变现有 browser toolset 的使用方式，helper 和测试链路也需要无收益地同步重写。

### 5. attach 与 launch 在同一线程内互斥，`browser__close` 在 attach 模式下只断开连接

引入 `browser__open(mode=attach)` 后，同一 thread-scoped session 内必须清楚地区分会话来源，并保证只存在一个活动 session。再次 open 时，系统应先关闭并替换当前 session，而不是允许一个线程同时持有自启动浏览器和 attach 连接。

相应地，`browser__close` 在 attach 模式下只负责关闭当前页面句柄、context client 或 sidecar 连接，而 SHALL NOT 杀掉外部浏览器进程。

Alternative considered:

- attach 后仍然让 `browser__close` 尝试关闭整个外部浏览器。
  Rejected，因为 attach 目标可能是用户自己正在使用的浏览器实例，不能把它当成受本系统完全托管的进程。

### 6. 继续禁止隐式附着默认 Profile，但允许 attach 到用户显式开启 remote debugging 的实例

首版 browser change 明确禁止直接复用默认 Chrome Profile，这条边界仍然保留。CDP attach 不等于放开这条限制；允许的是“用户显式提供了一个已开启 remote debugging 的现有 Chromium endpoint”，而不是“系统自己去抓用户日常浏览器”。

这能同时满足两个要求：

- 支持复用用户已经登录好的实例
- 不把 attach 变成不透明的默认行为

## Risks / Trade-offs

- [cookies 只覆盖 HTTP cookie，不覆盖更完整的站点状态] -> 首版在 spec 中明确只承诺 cookies，避免把能力说大；后续确有需要再独立扩到 storage state。
- [attach 到外部浏览器后页面/标签页状态不可控] -> 要求 `browser__open(mode=attach, ...)` 返回当前接管页面的信息，并让后续 snapshot 重新建立观察基线。
- [错误的 cookies 文件可能导致 launch 初始化失败或登录态异常] -> 缺少文件时允许首次 launch 正常继续；格式非法或内容损坏时需要显式报错，而不是静默忽略。
- [attach endpoint 不可达时容易让实现偷回退到 launch 模式] -> spec 明确禁止静默回退，连接失败必须直接报错。
- [close 自动导出会把“关闭浏览器”和“持久化状态”耦合在一起] -> 需要在 close 结果里明确反馈是否执行了自动导出，以及导出是否成功，避免调试时丢失状态信息。

## Migration Plan

1. 扩展 browser runtime 配置与 sidecar 协议，补齐 cookies 状态文件、自动注入/自动导出 flag、统一 `browser__open` 和手动 cookies 导出动作。
2. 在 Node sidecar 中实现统一 open 路径，以及 launch 模式下的 cookies 自动注入、close 自动导出和显式手动导出。
3. 在 Rust `service/session/tool` 层补齐 `browser__open`、默认 lazy open 等价路径、attach/launch 替换逻辑和 close 结果表达。
4. 在 `command` / hidden helper / script 验证路径中补齐手动 cookies 导出和 open 参数化验证入口。
5. 补齐单元测试与真实链路 smoke，覆盖自动 cookies 复用和 attach 到已有 endpoint 两条主路径。

Rollback strategy:

- 删除新增的 config 字段、`browser__open` 扩展语义、cookies 生命周期处理和 attach/open 相关逻辑即可；现有 launch-only browser toolset 语义仍可保留为回退基线。

## Open Questions

- 自动 cookies 注入时，是否对同名同域 cookie 做覆盖，其余保留；还是每次都先清空再整批写入。
- `browser__open(mode=attach, ...)` 成功后默认接管哪个 page/tab：最后一个活跃页面、当前前台页面，还是新建页面；规范要求返回页面摘要，但具体选择策略仍可在实现时细化。
- 手动 cookies 导出 Command 的稳定名称是否直接归入现有 slash command 体系，还是继续先以 hidden helper 入口落地后再决定对外命名。
- 如果 close 已经完成浏览器释放，但 cookies 自动导出失败，最终结果应该呈现为“关闭成功但导出失败”的部分成功，还是整体按失败返回；这需要在实现阶段进一步定细则。
