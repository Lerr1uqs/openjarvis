# B 站首页持久化验证

这个目录用于验证“首次非 headless 登录，第二次自动复用登录态”的场景，并额外输出首页 ARIA snapshot。

## 目录结构

- [browser_bilibili_persist_observe.rs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/scripts/browser_bilibili_persist_observe.rs)
  B 站首页访问与 cookies 持久化验证脚本。
- `.openjarvis/browser/bilibili-persist/browser-cookies.json`
  当前脚本落盘的 cookies-only 状态文件，格式与当前 browser cookies state 文件保持一致。
- `runs/`
  每次执行会创建一个新的运行目录，存放截图、普通 snapshot、ARIA snapshot 和元信息。

`.openjarvis/browser/` 保存 Browser 持久化状态文件；当前这个脚本只会写 cookies JSON。`.openjarvis/browser/` 和 `runs/` 默认作为本地验证产物保留，不纳入版本控制，避免把登录态 cookies 或包含账号信息的截图误提交到仓库。

## 两阶段验证

第一阶段，非 headless 手动登录：

```bash
cargo run --bin browser_bilibili_persist_observe -- login
```

行为说明：

- 打开 `https://www.bilibili.com/`
- 默认不加载历史 state，保证首次验证是干净会话
- 你在浏览器完成登录后，回终端按回车
- 脚本会导出 cookies 到 `.openjarvis/browser/bilibili-persist/browser-cookies.json`
- 同时采集一份首页截图、普通 snapshot 和 ARIA snapshot

第二阶段，复用持久化状态再次访问：

```bash
cargo run --bin browser_bilibili_persist_observe -- capture
```

行为说明：

- 启动一个全新浏览器上下文
- 自动从 `.openjarvis/browser/bilibili-persist/browser-cookies.json` 加载 cookies
- 再次访问 B 站首页
- 输出新的截图、普通 snapshot、ARIA snapshot 和 `cookies_loaded`

注意：

- 当前复用链路只恢复 cookies
- 不会恢复 `localStorage`、`sessionStorage`、`IndexedDB`
- 如果某个站点登录态依赖这些存储，仅凭当前 cookies 文件可能无法完整恢复

如果你想在 `login` 模式下也复用旧 state，可以显式加：

```bash
cargo run --bin browser_bilibili_persist_observe -- login --reuse-state
```

如果你只想在终端环境快速采集而不依赖已有登录态，可以执行：

```bash
cargo run --bin browser_bilibili_persist_observe -- capture --headless
```

## 观察重点

你可以重点对比两次运行目录里的：

- `page-metadata.json` 中的 `cookies_loaded`
- `browser-snapshot.txt`
- `aria-snapshot.yaml`
- `bilibili-homepage.png`

如果第二次运行已经保持登录态，通常截图和页面 ARIA 结构会与未登录首页有明显差异。
