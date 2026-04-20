# Obsidian CLI Smoke Notes

## 背景

本地环境里 Obsidian CLI 需要依附已经启动的桌面 app，当前暂不支持 headless。

## 观察

- 需要先在桌面 Obsidian 里手动打开目标 vault，再执行 CLI。
- CLI 更适合做 `read`、`search`、`files`、`create` 这类受控动作。
- 如果当前打开的是别的 vault，CLI 可能返回成功退出码但读到错误内容，因此 preflight 需要校验读出的 `AGENTS.md` 是否匹配目标 vault。

## 验证点

- app 当前打开的是否就是目标 vault。
- CLI 是否能读取 `index.md` 和 `AGENTS.md`。
- 写入 wiki 页面后，索引是否能及时反映新条目。
