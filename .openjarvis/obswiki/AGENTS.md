# Obswiki Vault Instructions

当前 vault 根目录: `./.openjarvis/obswiki`

## 目录职责

- `raw/`: 只存放导入后的 markdown 原文，后续不改写。
- `wiki/`: 存放整理后的知识页和操作说明。
- `schema/`: 存放页面模板、更新规则和字段约束。
- `index.md`: 维护 demo vault 的入口索引。

## 工作约束

- 对受管页面的读取、搜索、写入、更新都走 `obswiki` 工具。
- 先搜再读，不只根据索引或标题直接回答。
- `raw/` 是不可变层，补充资料要新建到 `wiki/` 或重新导入 raw。
- `index.md` 由系统维护；demo 数据变更后允许人工同步一次。

## Demo 范围

- 当前 demo 资料围绕 `OpenJarvis + Obsidian CLI + Rust agent runtime`。
- `raw/` 放外部来源风格笔记。
- `wiki/` 放可直接拿来问答和总结的整理页。
- `schema/` 放模板和更新规则，方便后续继续扩 demo。
