---
title: OpenJarvis Obswiki Overview
page_type: overview
links:
  - raw/obsidian-cli-smoke-notes.md
  - raw/openjarvis-subagent-notes.md
  - wiki/cli-test-workflow.md
source_refs:
  - raw/obsidian-cli-smoke-notes.md
  - raw/openjarvis-subagent-notes.md
---
# OpenJarvis Obswiki Overview

## 一句话结论

`obswiki` 是 OpenJarvis 里专门面向 Obsidian vault 的子代理，负责在受控边界内完成检索、阅读和知识页维护。

## 关键事实

- 运行时只会通过 `obsidian` CLI 连接已运行的桌面 app，不负责启动或关闭 Obsidian。
- `raw/` 存原始资料，`wiki/` 存整理页，`schema/` 存模板和规则。
- child thread 初始化时会收到 vault 状态、`AGENTS.md` 和 `index.md` 正文。

## 推荐操作路径

1. 先用 `obswiki_search` 找候选页面。
2. 再用 `obswiki_read` 读取命中的 `wiki/` 或 `raw/` 页面。
3. 需要沉淀结果时，用 `obswiki_write` 或 `obswiki_update` 回写 `wiki/`。
