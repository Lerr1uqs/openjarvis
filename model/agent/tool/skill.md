# Skill

## 定位

- `skill` 是本地知识包的发现和按需加载层。
- 它补充的是任务说明和操作手册，不是直接执行动作的工具。

## 边界

- 负责扫描本地 skill、维护 manifest、加载 skill 正文和引用文件、暴露 `load_skill` 工具。
- 不负责真实动作执行，不替代 MCP，也不替代 builtin tools。

## 关键概念

- `SkillManifest`
  skill 的索引元数据。
- `LoadedSkill`
  实际加载后的 skill 内容。
- `LoadedSkillFile`
  skill 引用到的附属文件内容。
- `SkillRegistry`
  本地 skill 的发现与缓存入口。
- `load_skill`
  模型按需加载 skill 的入口工具。

## 核心能力

- 从本地 `.skills` 根目录扫描 `SKILL.md`。
- 支持 enable / disable / restrict_to 等启用控制。
- 在真正需要时再把 skill 内容装进上下文，避免常驻膨胀。
- 解析 skill 正文里引用的相对文件并一起加载。

## 使用方式

- skill 适合承载流程说明、领域规范、额外约束。
- 如果一个能力需要读写外部世界，应优先做成 tool；如果只是补充做事说明，再做成 skill。
