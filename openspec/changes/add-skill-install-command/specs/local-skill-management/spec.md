## ADDED Requirements

### Requirement: 系统 SHALL 默认从工作区 `.openjarvis/skills` 发现本地 skill

系统 SHALL 把当前工作区 `.openjarvis/skills` 作为默认本地 skill 根目录。未显式传入自定义 roots 时，skill registry 与相关运行时装配 SHALL 基于该目录发现 `SKILL.md`。

#### Scenario: 默认 skill 目录位于工作区 `.openjarvis/skills`

- **WHEN** 系统在未显式指定 skill roots 的情况下创建 skill registry 或 agent runtime
- **THEN** 它会扫描当前工作区 `.openjarvis/skills`
- **THEN** 它 SHALL NOT 再默认扫描旧目录 `.skills`

### Requirement: 系统 SHALL 提供公开的本地 skill 安装命令

系统 SHALL 提供公开 CLI 命令 `openjarvis skill install <name>`，用于把受支持的 curated skill 安装到当前工作区的 `.openjarvis/skills/<name>/SKILL.md`。

#### Scenario: 安装受支持的 curated skill

- **WHEN** 用户执行 `openjarvis skill install acpx`
- **THEN** 系统会创建当前工作区 `.openjarvis/skills/acpx/`
- **THEN** 系统会下载并校验 `acpx` 的 `SKILL.md`
- **THEN** 系统会把校验通过的 skill 写入 `.openjarvis/skills/acpx/SKILL.md`

### Requirement: 系统 SHALL 提供公开的本地 skill 卸载命令

系统 SHALL 提供公开 CLI 命令 `openjarvis skill uninstall <name>`，用于从当前工作区删除对应本地 skill 目录。

#### Scenario: 卸载已安装的本地 skill

- **WHEN** 用户执行 `openjarvis skill uninstall acpx`
- **THEN** 系统会删除当前工作区 `.openjarvis/skills/acpx/`
- **THEN** `.openjarvis/skills/acpx/SKILL.md` 不再存在

### Requirement: 系统 SHALL 拒绝未知 curated skill 名称

系统 SHALL 在安装命令收到未知 skill 名称时返回明确错误，而不是静默跳过或创建空目录。

#### Scenario: 安装未知 skill 名称失败

- **WHEN** 用户执行 `openjarvis skill install missing-skill`
- **THEN** 命令返回失败
- **THEN** 错误信息指出该 skill 不在受支持的 curated registry 中

### Requirement: 系统 SHALL 只把结构合法的 skill 写入默认目录

系统 SHALL 在安装期间校验下载得到的 `SKILL.md` frontmatter。若校验失败，系统 SHALL NOT 保留损坏的最终 skill 文件。

#### Scenario: 远端返回非法 SKILL.md

- **WHEN** 安装命令下载到缺少合法 frontmatter 的 skill 内容
- **THEN** 安装命令返回失败
- **THEN** `.openjarvis/skills/<name>/SKILL.md` 不会以损坏内容落盘

### Requirement: 系统 SHALL 让新安装的 skill 被默认运行时直接发现

系统 SHALL 让通过 `openjarvis skill install <name>` 写入默认目录的 skill，在后续默认 skill registry 扫描和 `--load-skill <name>` 启动路径中可直接被发现。

#### Scenario: 安装后的 skill 可被默认 registry 发现

- **WHEN** 用户先安装 `acpx`，随后使用默认 skill roots 启动运行时
- **THEN** skill registry 能列出 `acpx`
- **THEN** `--load-skill acpx` 不会因为默认路径错误而失败
