# agent/tool/skill 模块总览

## 作用

`skill/` 负责本地 skill 的发现、索引、按需加载与 Agent 暴露。它解决的问题不是“执行什么工具”，而是“给 Agent 动态补充什么经验说明或工作手册”。

## 子模块

- `registry.rs`
  skill 注册表。负责扫描 skill 文件、解析元信息、索引内容、加载引用文件。
- `tool.rs`
  `load_skill` 工具。负责让模型在运行时按需加载某个 skill。

## 核心概念

- `Skill`
  一份可被 Agent 按需加载的任务知识包，通常描述做事方法、约束和附加参考文件。
- `SkillManifest`
  skill 的索引元数据，用于快速列出有哪些 skill 可用。
- `LoadedSkill`
  已经实际加载到运行时里的 skill 内容。
- `LoadedSkillFile`
  skill 体内引用到的附属文件内容。
- `SkillRegistry`
  本地 skill 的发现与缓存入口。
- `load_skill`
  暴露给 Agent 的加载动作，让模型在真正需要时再把 skill 引入上下文。

## 设计意图

- skill 更像“操作说明书”或“领域攻略”，不是普通函数调用。
- skill 的核心目标也是减少无关上下文常驻，把额外知识延后到需要时再加载。

## 边界

- skill 不直接替代 MCP 或内建工具。
- skill 偏知识与流程提示，工具偏真实动作执行。
