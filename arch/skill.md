skill提供的能力

load_skills 当前默认从工作区本地路径加载，未来可以再扩展其他路径或配置化

- 当前实现默认 ./.openjarvis/skills/
- 未来可扩展 ~/.openjarvis/skills/
- 未来可扩展 ~/.openjarvis/installed_skills/
- 未来可扩展其他指定路径

fetch_remote 从clawhub等地方获取

对skill进行增删改查 或者disable/enable

skill暴露tool(load_skill()) 让agent能够调用 

ut中对skill的各种malform进行ut 能不能正确parse

skill的稳定格式 frontmatter的解析器

skill 转换给agent的转换器？to_prompt?

热更新？

审批和权限机制 除了默认的 都暂时不运行load 所以要有提供给外面trusted approve等接口


# Anthropic Agent Skills 极简协议说明

## 1) 它是什么

Anthropic 的 Agent Skills 本质上是**一组放在文件夹里的能力包**：用 `SKILL.md` 作为入口，再按需附带脚本、说明文档、模板和资源文件。Agent 先只预加载每个 skill 的 `name` 和 `description`，命中后再读取完整 `SKILL.md`，再继续按需读取其他文件。这种方式的核心是**渐进加载（progressive disclosure）**。citeturn228557search0turn228557search1

给其他 agent 的一句话定义：

> Skill = 一个带 `SKILL.md` 的目录；目录里放“说明 + 代码 + 资源”，agent 先看元信息，命中后再按需把完整内容和附属文件加载进上下文。citeturn228557search0

---

## 2) 它的几个关键特征

- **目录协议而不是单段 prompt**：skill 是文件夹，不是一个字符串提示词。入口文件是 `SKILL.md`。citeturn228557search0turn228557search1
- **最小元信息触发**：启动时只需要预加载 `name` 和 `description`，用于路由和匹配。citeturn228557search0
- **渐进加载**：匹配后才读 `SKILL.md`；如果 `SKILL.md` 里再引用其他文件，agent 再继续按需读取。citeturn228557search0turn228557search1
- **可组合**：多个 skills 可以叠加使用。citeturn228557search1
- **可移植**：Anthropic 公开把 Agent Skills 描述为跨 Claude apps、Claude Code、Agent SDK、API 可复用的格式，并在 2025-12-18 说明其已作为 open standard 发布。citeturn228557search0turn228557search1
- **可执行**：skill 可以携带脚本；而在 Anthropic API 侧，skills 依赖 code execution tool 这类执行环境去运行 Bash、文件编辑、代码生成等动作。citeturn228557search1turn421383search0

---

## 3) 公开可确认的最小协议面

目前从 Anthropic 公开材料里，**可以明确确认**的最小兼容面是：

### 目录结构（最小版）

```text
my-skill/
├── SKILL.md
├── reference.md        # 可选
├── forms.md            # 可选
├── scripts/...         # 可选
└── assets/...          # 可选
```

### `SKILL.md` 入口要求

`SKILL.md` 必须以 YAML frontmatter 开头，且至少包含：

```yaml
---
name: pdf
description: guidance and resources for working with pdfs and forms
---
```

随后正文写这个 skill 的说明、步骤、引用其他文件的方法。Anthropic 明确说明：`SKILL.md` 必须包含 `name` 和 `description` 这两个必需字段，agent 启动时先加载这两个字段，再在命中时读取正文。citeturn228557search0

### 加载顺序

1. 扫描已安装 skills  
2. 读取每个 `SKILL.md` 的 `name` / `description`  
3. 把这批元信息放进系统上下文或路由层  
4. 收到用户请求后，做 skill 匹配  
5. 命中 skill 后，再读取完整 `SKILL.md`  
6. 如正文引用别的文件，再按需读取这些文件  
7. 如 skill 需要脚本执行，则交给 code execution / shell / 文件编辑能力运行。citeturn228557search0turn421383search0

---

## 4) 集成到你自己 agent 框架时，建议把它实现成什么

你可以把 Anthropic Skill 当成一个**文件系统协议 + 两阶段加载协议**：

### 抽象对象

```ts
type SkillManifest = {
  name: string
  description: string
  path: string
}

type LoadedSkill = {
  manifest: SkillManifest
  body: string          // SKILL.md 正文
  referencedFiles: string[]
}
```

### 两阶段策略

#### 阶段 A：索引期
只做这些事：

- 递归扫描 marketplace / plugins / local skills 目录
- 找到所有 `SKILL.md`
- 解析 frontmatter
- 建一个内存索引：`name + description + path`

这一层只为“**发现与路由**”服务。

#### 阶段 B：命中后加载
当用户请求进入时：

- 用用户输入对 `name + description` 做匹配
- 选中 0~N 个 skill
- 读取对应 `SKILL.md` 全文
- 解析正文里引用的文件路径
- 再按需把这些文件内容喂给模型，或挂成可读资源
- 如果 skill 需要脚本，交给 agent 的工具层执行

这正是 Anthropic 公开描述的 progressive disclosure 模式。citeturn228557search0turn228557search1

---

## 5) 一个够用的兼容实现骨架（Node.js）

下面这个实现不是 Anthropic 官方 SDK 代码，而是**兼容其公开协议思路的最小加载器**，适合集成到自己的 agent 框架。公开资料确认了 skill 的目录入口、frontmatter、渐进加载和可执行特性；下面的具体代码组织是工程实现建议。citeturn228557search0turn228557search1turn421383search0

```ts
import fs from "node:fs"
import path from "node:path"
import matter from "gray-matter"

export type SkillManifest = {
  name: string
  description: string
  skillDir: string
  skillFile: string
}

export type SkillFull = SkillManifest & {
  body: string
}

export class SkillRegistry {
  private manifests: SkillManifest[] = []

  constructor(private roots: string[]) {}

  scan(): SkillManifest[] {
    const found: SkillManifest[] = []

    for (const root of this.roots) {
      walk(root, (file) => {
        if (path.basename(file) !== "SKILL.md") return

        const raw = fs.readFileSync(file, "utf8")
        const parsed = matter(raw)
        const name = String(parsed.data?.name || "").trim()
        const description = String(parsed.data?.description || "").trim()

        if (!name || !description) return

        found.push({
          name,
          description,
          skillDir: path.dirname(file),
          skillFile: file,
        })
      })
    }

    this.manifests = found
    return found
  }

  list(): SkillManifest[] {
    return this.manifests
  }

  match(userQuery: string, topK = 3): SkillManifest[] {
    const q = userQuery.toLowerCase()

    return [...this.manifests]
      .map((m) => {
        const hay = `${m.name} ${m.description}`.toLowerCase()
        let score = 0
        for (const token of q.split(/\s+/)) {
          if (token && hay.includes(token)) score += 1
        }
        if (hay.includes(q)) score += 3
        return { manifest: m, score }
      })
      .filter((x) => x.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, topK)
      .map((x) => x.manifest)
  }

  load(skill: SkillManifest): SkillFull {
    const raw = fs.readFileSync(skill.skillFile, "utf8")
    const parsed = matter(raw)
    return {
      ...skill,
      body: parsed.content.trim(),
    }
  }
}

function walk(dir: string, onFile: (file: string) => void) {
  if (!fs.existsSync(dir)) return
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name)
    if (entry.isDirectory()) walk(p, onFile)
    else if (entry.isFile()) onFile(p)
  }
}
```

---

## 6) 在 agent runtime 里怎么接

```ts
const registry = new SkillRegistry([
  "./skills",
  "./marketplace-skills",
  path.join(process.env.HOME || "", ".agent/skills"),
])

registry.scan()

export async function buildSystemContext(userTask: string) {
  const matched = registry.match(userTask, 3)

  const skillHeaders = registry.list().map((s) => ({
    name: s.name,
    description: s.description,
  }))

  const loadedSkills = matched.map((s) => registry.load(s))

  return {
    skillHeaders,   // 常驻：只放 name/description
    loadedSkills,   // 命中后：放完整正文
  }
}
```

建议模型输入分两层：

### 常驻层
只放：

```json
[
  { "name": "pdf", "description": "guidance and resources for working with pdfs and forms" },
  { "name": "excel", "description": "work with spreadsheets and formulas" }
]
```

### 命中层
只在命中时追加：

- `SKILL.md` 正文
- skill 引用的补充文件
- skill 允许调用的脚本或工具说明

---

## 7) 如果你还想兼容“市场上的 skills”，最重要的是这 5 条

1. **把 skill 看成文件夹包，不要看成 prompt 字符串。**  
2. **强制要求 `SKILL.md` + YAML frontmatter。**  
3. **frontmatter 至少兼容 `name` 和 `description`。**  
4. **实现“先索引元信息、后按需全文加载”的两阶段加载。**  
5. **给 skill 留文件读取和代码执行通道，否则很多 skill 只有说明没法落地。** Anthropic 公开资料明确把 skills 与代码执行环境联动描述。citeturn228557search0turn228557search1turn421383search0

---

## 8) 需要注意的边界

- **公开资料能确认的是“格式和加载思想”**，不是完整的、逐字段、逐接口的正式 RFC。  
- Anthropic 新闻稿提到 Developer Platform 有 `/v1/skills` 端点用于 skill 版本管理，但我当前检索到的公开文档页里，**没有拿到完整接口 schema 页面**；因此上面的代码实现应理解为**兼容公开协议思想的工程落地版本**，而不是对未公开细节的逐字复刻。citeturn228557search1
- 如果你的目标是“兼容市面上的 skill 包”，最稳妥做法是把 `SKILL.md` 协议做成**宽松解析**：  
  - 必需：`name`、`description`  
  - 其余字段：保留但不强依赖  
  - 未知字段：透传

---

## 9) 给其他 agent 的超短总结

```text
Anthropic Skill 是一个带 SKILL.md 的目录协议。
Agent 先只读取每个 skill 的 name/description 做匹配，
命中后再加载完整 SKILL.md 和它引用的文件，
需要执行时再调用本地代码/文件工具。
```

这就是最值得兼容的核心。
