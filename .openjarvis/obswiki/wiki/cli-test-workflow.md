---
title: CLI Test Workflow
page_type: workflow
links:
  - wiki/openjarvis-obswiki-overview.md
  - raw/obsidian-cli-smoke-notes.md
source_refs:
  - raw/obsidian-cli-smoke-notes.md
---
# CLI Test Workflow

## 目标

用一条最短命令验证 `internal-obswiki` 能否在 Obsidian app 已手动启动的前提下装载 demo vault，并由子代理读取当前知识库上下文。

## 执行前检查

- 当前工作目录是仓库根目录。
- `config.yaml` 里已启用 `agent.tool.obswiki.enabled: true`。
- `obsidian` 在当前 shell 的 `PATH` 里可用。
- 目标 vault 已经在桌面 Obsidian 中打开；agent 不会代替你启动 app。

## 推荐 smoke prompt

- `总结当前 demo vault 里有哪些资料，以及最适合先读哪几篇。`
- `根据当前 demo vault，说明 obswiki 子代理的工作边界。`
- `列出 raw/wiki/schema 三层分别适合放什么内容。`

## 预期现象

- 如果 Obsidian app 没有提前启动，命令会直接报错并提示手动启动后重试。
- 子代理会基于 demo vault 输出回答，而不是走空知识库。
- 如果后续写回 wiki 页面，`index.md` 应会出现新条目。
