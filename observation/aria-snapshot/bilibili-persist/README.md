# B 站首页持久化验证

这个目录用于验证“首次非 headless 登录，第二次自动复用登录态”的场景，并额外输出首页 ARIA snapshot。

## 目录结构

- [capture_bilibili_persist.mjs](/home/pc/proj/self-projs/openjarvis-worktrees/browser-automation/observation/aria-snapshot/bilibili-persist/capture_bilibili_persist.mjs)
  B 站首页访问与 cookies 持久化验证脚本。
- `state/browser-cookies.json`
  持久化 cookies 文件，格式与当前 browser cookies state 文件保持一致。
- `runs/`
  每次执行会创建一个新的运行目录，存放截图、ARIA snapshot 和元信息。

`state/` 和 `runs/` 默认作为本地验证产物保留，不纳入版本控制，避免把登录态 cookies 或包含账号信息的截图误提交到仓库。

## 两阶段验证

第一阶段，非 headless 手动登录：

```bash
node observation/aria-snapshot/bilibili-persist/capture_bilibili_persist.mjs login
```

行为说明：

- 打开 `https://www.bilibili.com/`
- 默认不加载历史 state，保证首次验证是干净会话
- 你在浏览器完成登录后，回终端按回车
- 脚本会导出 cookies 到 `state/browser-cookies.json`
- 同时采集一份首页截图和 ARIA snapshot

第二阶段，复用持久化状态再次访问：

```bash
node observation/aria-snapshot/bilibili-persist/capture_bilibili_persist.mjs capture
```

行为说明：

- 启动一个全新浏览器上下文
- 自动从 `state/browser-cookies.json` 加载 cookies
- 再次访问 B 站首页
- 输出新的截图、ARIA snapshot 和 `cookies_loaded`

如果你想在 `login` 模式下也复用旧 state，可以显式加：

```bash
node observation/aria-snapshot/bilibili-persist/capture_bilibili_persist.mjs login --reuse-state
```

如果你只想在终端环境快速采集而不依赖已有登录态，可以执行：

```bash
node observation/aria-snapshot/bilibili-persist/capture_bilibili_persist.mjs capture --headless --allow-missing-state
```

## 观察重点

你可以重点对比两次运行目录里的：

- `page-metadata.json` 中的 `cookies_loaded`
- `aria-snapshot.yaml`
- `bilibili-homepage.png`

如果第二次运行已经保持登录态，通常截图和页面 ARIA 结构会与未登录首页有明显差异。
