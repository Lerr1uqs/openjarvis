# Browser 工具能力文档

本文档描述当前系统提供的浏览器自动化工具能力。

## 概述

Browser 工具集基于 Node.js Playwright sidecar 实现，支持通过 Chrome DevTools Protocol (CDP) 控制浏览器会话。

## 工具列表

| 工具名称 | 功能描述 |
|---------|---------|
| `browser__open` | 打开或替换当前线程范围的浏览器会话（支持 Launch 或 Attach 模式） |
| `browser__navigate` | 在当前会话中导航到指定 URL 并返回页面快照 |
| `browser__console` | 获取浏览器控制台日志（Console logs） |
| `browser__errors` | 获取页面错误和请求失败信息 |
| `browser__requests` | 获取网络请求记录（Network requests） |
| `browser__snapshot` | 捕获页面快照（包含可交互元素的引用 refs） |
| `browser__click_ref` | 点击快照中指定 ref 的元素 |
| `browser__click_match` | 通过语义匹配条件点击元素（无需提前知道 ref） |
| `browser__type_ref` | 向指定 ref 的元素输入文本 |
| `browser__type_match` | 通过语义匹配条件向元素输入文本 |
| `browser__screenshot` | 截取当前页面截图并保存 |
| `browser__close` | 关闭当前线程的浏览器会话并释放资源 |

## 会话模式

### Launch 模式
启动新的浏览器实例（默认模式）。

### Attach 模式
连接到已运行的 Chromium 实例，通过 `cdp_endpoint` 参数指定 CDP 端点地址（如 `http://127.0.0.1:9222`）。


## 诊断查询参数

用于日志查询类工具的通用参数：

| 参数名 | 说明 |
|-------|------|
| `limit` | 限制返回记录数量 |
| `failed_only` | 仅返回失败的记录（`browser__requests` 专用） |

## 实现位置

- **工具实现**: `src/agent/tool/browser/tool.rs`
- **协议定义**: `src/agent/tool/browser/protocol.rs`
- **会话管理**: `src/agent/tool/browser/session.rs`
- **服务层**: `src/agent/tool/browser/service.rs`
- **Sidecar 脚本**: `scripts/browser_sidecar.mjs`
