# Rust 如何操控 Chrome 浏览器完成任务

## 1. 目标

这份文档回答三个问题：

1. Rust 能不能稳定地操控 Chrome 去完成真实任务。
2. Rust 生态里有哪些主流路线，它们分别适合什么场景。
3. 对 openjarvis 来说，哪条路线最合适，后续应该怎么落地。

本文调研时间为 **2026-03-25**。

## 2. 先给结论

- 如果目标是“在 Rust 里直接控制 Chrome/Chromium 完成点击、输入、截图、抓取、导出 PDF、执行 JS、监听网络”等底层自动化任务，**优先推荐 `chromiumoxide`**。
- 如果目标是“给 LLM / Agent 一个可稳定执行网页任务的浏览器控制能力”，核心抽象不应该是“整页原始 HTML + CSS selector”，而应该是 **CDP 连接 + Playwright 高阶动作 + 文本化 UI snapshot + ref 驱动 act**。
- 如果目标是“尽量走标准协议、未来可切 Firefox/Edge、或者需要接入现成 Selenium/Grid 基础设施”，**优先考虑 `fantoccini` 或 `thirtyfour`**。
- 如果目标是“自动化复杂站点，追求更强的稳定性、生态成熟度、对 iframe / 下载 / 上传 / 登录流 / 反爬细节更友好”，**工程上更推荐 Node.js Playwright sidecar，Rust 只负责调度**。
- 对 **openjarvis** 来说，最合理的路线不是只押注一种方案，而是：
  - **Agent 主线**: 做一个基于 `Playwright sidecar / MCP browser server` 的浏览器控制服务。
  - **Rust 补充能力**: 保留 `chromiumoxide` 作为底层 CDP 能力、原型验证或特定 Chrome 能力补充。

## 3. Rust 控 Chrome 的三条主路线

### 3.1 CDP: 直接走 Chrome DevTools Protocol

代表库：

- `chromiumoxide`
- `headless_chrome`

特点：

- 直接和 Chrome / Chromium 说话，协议层更贴近浏览器内部能力。
- 截图、PDF、执行 JS、网络事件、目标页管理、DevTools 能力通常更强。
- 强绑定 Chromium 家族，不是跨浏览器标准接口。

适合：

- openjarvis 这种“浏览器就是一个工具能力”的 agent 场景。
- 需要拿到更底层页面控制能力，而不只是“测试页面能不能点开”。

### 3.2 WebDriver: 走标准化自动化协议

代表库：

- `fantoccini`
- `thirtyfour`

特点：

- 标准化、跨浏览器、和 Selenium/ChromeDriver 体系兼容。
- 一般需要先启动 `chromedriver` 或 Selenium Server。
- 能力更偏“页面自动化测试”，对于 Chrome 专属能力不如 CDP 直接。

适合：

- 你希望实现尽量标准化。
- 未来可能切换浏览器。
- 团队已有 Selenium / Grid 基础设施。

### 3.3 Playwright: 不直接在 Rust 内实现，而是让 Rust 调度外部浏览器服务

特点：

- Playwright 官方主生态不在 Rust。
- 在工程实践里，常见做法是：
  - Rust 启动 Node sidecar。
  - Rust 通过 stdio / HTTP / WebSocket / MCP 调用浏览器动作。
- Playwright 在实现层通常仍然建立在浏览器原生调试能力之上，适合承担 CDP 之上的高层动作编排。
- 这种方式在复杂网页场景里往往更稳，因为 Playwright 在浏览器自动化领域的工程成熟度非常高。

适合：

- 复杂真实站点自动化。
- 后续要支持录屏、trace、复杂等待、下载、上传、多个 browser context。
- 希望把“浏览器能力”独立成一个可替换组件。

### 3.4 面向 Agent 的正确抽象: snapshot / screenshot / act

对人写脚本来说，直接拿 DOM 和 CSS selector 去操作页面是常见做法。  
但对模型驱动的浏览器控制来说，这通常不是最稳的抽象。

更合理的模型是：

1. 浏览器控制服务先接受 HTTP / MCP / 本地 RPC 请求。
2. 服务通过 CDP 连到 Chromium 系浏览器。
3. 在 CDP 之上用 Playwright 执行更高层的页面动作。
4. 服务把当前页面转换成 **文本化 UI snapshot**，并给可交互元素分配 `ref`。
5. 模型基于 `snapshot` 和 `screenshot` 理解页面，再发出 `click <ref>`、`type <ref>` 之类的动作。

这里的关键点不是“把整页原始 HTML 塞给模型”，而是：

- 给模型看经过裁剪和结构化的 **snapshot**
- 用 **screenshot** 提供视觉校验
- 用 **act** 接口执行动作
- 用 `ref` 编号定位元素，而不是让模型临时拼 CSS selector

这样做的好处：

- 避免原始 HTML 过长、噪声过多。
- 减少 selector 脆弱性。
- 更适合多轮观察-行动循环。
- 更容易记录、回放、调试和审计。

一个典型工作流如下：

1. `snapshot`
2. 从 snapshot 中拿到元素 `ref`
3. `click <ref>` / `type <ref>` / `highlight <ref>` / `screenshot --ref <ref>`
4. 页面变化后重新 `snapshot`
5. 继续下一轮动作

## 4. 当前 Rust 生态调研

以下版本和更新时间来自 crates.io，核对时间为 **2026-03-25**。

| crate | 协议 | 最新版本 | crates.io 更新时间 | 适合场景 | 主要问题 |
| --- | --- | --- | --- | --- | --- |
| `chromiumoxide` | CDP | `0.9.1` | `2026-02-25` | Rust 原生控制 Chrome，异步集成，拿 DevTools 能力 | 只偏 Chromium，编译和依赖较重 |
| `headless_chrome` | CDP | `1.0.21` | `2026-02-03` | 快速做单机抓取/截图/PDF | API 偏同步线程模型，和本项目 Tokio 风格不完全一致 |
| `fantoccini` | WebDriver | `0.22.1` | `2026-02-28` | 走标准协议、接口相对克制 | 需要单独维护 WebDriver 进程 |
| `thirtyfour` | WebDriver | `0.36.1` | `2025-07-06` | WebDriver 功能更全，API 更丰富 | 同样依赖 ChromeDriver / Selenium，Chrome 专属能力有限 |

### 4.1 `chromiumoxide`

优点：

- Tokio 异步模型，和 openjarvis 当前技术栈一致。
- 直接走 CDP，适合做“浏览器工具”而不是单纯 UI 测试。
- 支持启动 Chrome，也支持连已有浏览器实例。
- README 明确支持 headless / headful 模式。

缺点：

- Chromium 专属，不是跨浏览器中立方案。
- 编译成本偏高。
- 某些底层能力需要自己继续封装，不能指望像 Playwright 一样“一把梭”。

结论：

- **最适合 openjarvis 作为第一版 Rust 浏览器能力实现。**

### 4.2 `headless_chrome`

优点：

- API 直观，截图/PDF/执行 JS 上手快。
- 直接走 CDP。

缺点：

- README 明确说明其 API 是同步风格，底层更多依赖线程，而不是 Tokio 异步模型。
- 对 openjarvis 这种本身就是 async runtime 的项目来说，集成体验不如 `chromiumoxide`。

结论：

- 适合做小工具、原型验证。
- **不建议作为 openjarvis 主线方案。**

### 4.3 `fantoccini`

优点：

- WebDriver 路线，标准化程度高。
- crate 更新活跃。
- API 较克制，适合做稳定的页面交互封装。

缺点：

- 需要额外运行 `chromedriver` 或 Selenium。
- 对截图、网络、PDF、DevTools 细节这类能力，天然不如 CDP 直接。

结论：

- 如果 openjarvis 后面要做跨浏览器兼容，这是值得保留的路线。
- 但如果当前目标明确是“操控 Chrome 完成任务”，优先级低于 `chromiumoxide`。

### 4.4 `thirtyfour`

优点：

- WebDriver 能力更全，等待、组件化封装、查询接口都比较完整。
- 更像面向自动化测试工程的完整工具箱。

缺点：

- 路线仍然是 WebDriver，Chrome 专属增强能力有限。
- 对 openjarvis 当前的目标来说，抽象层略重。

结论：

- 如果后续浏览器模块会演变成一套自动化测试基础设施，可以考虑。
- 作为当前 openjarvis 的浏览器执行层，不是最优先。

## 5. 官方资料里值得注意的几个事实

### 5.1 Chrome 提供了原生的 CDP 调试协议

Chrome DevTools Protocol 是 Chrome 官方公开的调试协议，很多浏览器自动化能力本质都建立在这层协议上。  
这意味着如果你只关心 Chrome/Chromium，直接使用 CDP 路线是合理的。

### 5.2 WebDriver 更标准，但不是最贴近 Chrome 特性的方案

ChromeDriver 官方文档说明它本质上是 WebDriver 服务端实现。  
如果你想要跨浏览器兼容、接 Selenium/Grid，这是优点；如果你想充分使用 Chrome 特性，这反而会多一层约束。

### 5.3 Chrome 团队已经在收紧远程调试开关的安全策略

Chrome 官方在 **2025-03-17** 发布了关于 remote debugging switches 的安全调整说明：  
从 **Chrome 136** 开始，如果你试图调试默认用户数据目录，`--remote-debugging-port` 和 `--remote-debugging-pipe` 不再按旧方式工作。

这件事直接影响实现方式：

- **不要直接附着到用户正在使用的默认 Chrome Profile。**
- 应该总是为自动化任务创建独立的 `user-data-dir`。
- 更推荐使用 **Chrome for Testing** 或专门的自动化浏览器实例。

### 5.4 Playwright 官方主生态依旧不是 Rust

Playwright 官方文档当前主要面向 Node.js / Python / Java / .NET。  
这意味着如果未来你想用 Playwright 的能力，工程上更现实的方式通常不是“在 Rust 里找一个不成熟绑定”，而是 **Rust 调一个外部 Playwright 服务**。

### 5.5 对 LLM 来说，HTML 往往不是最佳控制界面

这不是某个单一官方文档直接给出的结论，而是 Agent 浏览器控制的工程经验总结：

- 原始 HTML 信息量过大，而且包含大量对决策无帮助的噪声。
- CSS selector 对动态页面、重渲染、A/B 实验和样式改版很脆弱。
- LLM 更适合处理“结构化文本快照 + 编号引用 + 明确动作”的控制协议。

因此如果 openjarvis 要做的是 **Agent browser control**，应优先设计 **snapshot/ref/act** 协议，而不是把 DOM API 直接暴露给模型。

## 6. openjarvis 推荐路线

### 6.1 第一阶段: 先做浏览器控制服务，而不是先做 selector API

推荐实现：

- Rust 侧负责 tool 接口、任务调度、权限控制、超时、产物归档。
- 浏览器执行层优先使用 Playwright sidecar。
- sidecar 对 Chromium 建立 CDP 连接，并提供 snapshot / screenshot / act 这套高层语义。
- 不要把“CSS selector + 原始 HTML”作为给模型的默认接口。

建议的模块边界：

- `src/browser/mod.rs`
- `src/browser/session.rs`
- `src/browser/service.rs`
- `src/browser/protocol.rs`
- `src/browser/artifact.rs`
- `src/browser/tool.rs`

职责建议：

- `session.rs`: 管理单次浏览器任务生命周期、超时、临时目录。
- `service.rs`: 管理 sidecar 进程、HTTP / MCP 连接、健康检查、重试。
- `protocol.rs`: 定义 `snapshot`、`click(ref)`、`type(ref, text)`、`screenshot(ref)` 等结构化协议。
- `artifact.rs`: 存储 HTML、截图、PDF、console、network log。
- `tool.rs`: 对 Agent 暴露结构化工具接口，例如 `browser_snapshot`、`browser_click_ref`、`browser_type_ref`。

这样做的原因：

- 符合当前项目“模块化低耦合”的要求。
- Agent 看到的是稳定的动作协议，而不是脆弱的页面实现细节。
- 后续替换底层实现时，`tool.rs` 层可以尽量不变。

### 6.2 第二阶段: 再补 Rust 原生 CDP 能力

推荐方式：

- 保留 `chromiumoxide` 作为 Rust 原生 CDP 访问层。
- 用于实验性能力、性能敏感路径、或 sidecar 暂时没有覆盖的 Chrome 专属功能。
- 但这层更适合作为内部实现细节，不建议直接暴露给模型。

这和当前仓库已有的 sidecar 思路是一致的：

- 仓库已经有 Node 侧脚本。
- 项目本身也有 MCP/tool registry 设计。

因此中期演进路径很清晰：

- **Agent 浏览器控制** 走 Playwright sidecar / MCP browser server
- **底层补充能力** 走 Rust + `chromiumoxide`

### 6.3 推荐的对外动作集合

如果要把浏览器控制真正做成工具服务，推荐对外暴露下面这些能力族，而不是零散 selector API。

标签页管理：

- `tabs`
- `open`
- `focus`
- `close`

页面导航：

- `navigate`

观察能力：

- `snapshot`
- `screenshot`
- `pdf`
- `console`
- `errors`
- `requests`

操作能力：

- `click`
- `type`
- `press`
- `hover`
- `drag`
- `select`
- `fill`
- `upload`
- `dialog`
- `download`

状态和环境：

- `cookies`
- `storage`
- `offline`
- `headers`
- `credentials`
- `geo`
- `timezone`
- `locale`
- `device`

调试能力：

- `highlight`
- `trace_start`
- `trace_stop`
- `responsebody`

## 7. 一个浏览器任务的推荐执行流程

如果是 Agent 驱动浏览器，建议按下面的生命周期来做：

1. 创建任务级临时目录。
2. 创建独立 `user-data-dir`，绝不复用默认用户 Profile。
3. 启动浏览器控制服务，建立到 Chromium 的 CDP 连接。
4. 打开页面并 `navigate`。
5. 生成一次 `snapshot` 和必要的 `screenshot`。
6. 模型根据 snapshot 中的 `ref` 发出 `click(ref)`、`type(ref, text)` 等动作。
7. 页面变化后重新 `snapshot`，进入下一轮观察-行动循环。
8. 保存 snapshot、截图、PDF、console、requests、errors、trace 等产物。
9. 清理页面、关闭浏览器、删除临时目录。

最关键的工程原则：

- **隔离 profile**
- **显式等待**
- **任务超时**
- **产物留档**
- **失败可复现**
- **ref 驱动而不是 selector 驱动**
- **snapshot 优先而不是原始 HTML 优先**

## 8. `chromiumoxide` 最小示例

最小依赖可以从下面开始：

```toml
[dependencies]
anyhow = "1"
chromiumoxide = "0.9.1"
futures = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

运行前建议：

- 本机已安装 Chrome / Chromium，或者直接使用 Chrome for Testing。
- 不要复用默认用户 Profile。
- 如果要做可复现自动化，优先固定浏览器版本。

下面这个例子展示 Rust 如何启动 Chrome、打开页面、点击元素并提取 HTML。  
它适合说明 **底层 CDP 自动化** 怎么做，但不适合直接作为 LLM 的高层控制接口。

```rust
use anyhow::Result;
use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let (mut browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .with_head()
            .build()?,
    )
    .await?;

    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let page = browser.new_page("https://example.com").await?;
    let body = page.find_element("body").await?;
    body.click().await?;

    let html = page.content().await?;
    println!("{}", html.len());

    browser.close().await?;
    let _ = handler_task.await;
    Ok(())
}
```

如果只是做 Rust 内部底层封装，适合在这个基础上继续包装：

- `goto(url)`
- `click(selector)`
- `type(selector, text)`
- `wait_for(selector)`
- `extract_text(selector)`
- `screenshot(path)`
- `eval(script)`

如果是给模型暴露能力，更推荐包装成：

- `snapshot()`
- `click(ref)`
- `type(ref, text)`
- `press(ref, key)`
- `drag(source_ref, target_ref)`
- `screenshot(ref)`
- `highlight(ref)`

## 9. 如果必须走 WebDriver，推荐怎么做

如果你明确想走标准协议，建议优先在 Chrome 场景里这样启动：

1. 安装与 Chrome 版本匹配的 `chromedriver`。
2. 用独立端口启动 `chromedriver`，例如 `9515`。
3. Rust 侧用 `fantoccini` 或 `thirtyfour` 连接它。
4. 同样使用独立用户目录和任务级超时控制。

这种方案的优点是标准化；缺点是：

- 多一个进程要维护。
- 一部分 Chrome 专属能力需要额外绕路。
- 对 agent 场景常见的截图、产物留存、网络调试，不如 CDP 直达。

## 10. 对 openjarvis 的最终建议

如果只允许选一条主线，我的建议很明确：

- **现在就做基于 Playwright sidecar 的 snapshot/ref/act 浏览器控制服务。**

原因：

- 当前项目是 Tokio 异步栈。
- 当前项目要的是“工具能力”，不是“测试框架”。
- 当前项目架构文档已经提到浏览器与 Playwright，说明浏览器能力未来本来就会成为独立组件。
- 对 Agent 来说，稳定的控制协议比直接暴露 selector 更重要。
- Playwright 更适合承担复杂页面上的高层动作执行。
- CDP + Playwright + snapshot/ref 的抽象更贴近真正的浏览器控制服务。

同时应该从第一天就留出抽象层，避免后续被单一 crate 绑死：

- 对外暴露统一 `BrowserTool` 接口。
- 默认接口围绕 `snapshot`、`ref` 和 `act` 设计。
- 内部实现允许未来切换 Playwright、CDP、甚至远程 browser server。
- 不直接把第三方 crate 类型暴露到业务层。

## 11. 参考资料

- Chrome DevTools Protocol: https://chromedevtools.github.io/devtools-protocol/
- ChromeDriver 官方文档: https://developer.chrome.com/docs/chromedriver/
- Chrome 官方远程调试安全调整说明（2025-03-17）: https://developer.chrome.com/blog/remote-debugging-port
- Chrome for Testing 说明: https://developer.chrome.com/blog/chrome-for-testing/
- Chrome for Testing 可用版本面板: https://googlechromelabs.github.io/chrome-for-testing/
- Playwright 官方文档: https://playwright.dev/docs/intro
- Playwright `connectOverCDP`: https://playwright.dev/docs/api/class-browsertype#browser-type-connect-over-cdp
- `chromiumoxide`: https://github.com/mattsse/chromiumoxide
- `fantoccini`: https://github.com/jonhoo/fantoccini
- `thirtyfour`: https://github.com/Vrtgs/thirtyfour
- `headless_chrome`: https://github.com/rust-headless-chrome/rust-headless-chrome
