## 1. Spec 与默认路径收敛

- [x] 1.1 新增 `local-skill-management` spec，说明默认 skill 根目录与安装命令行为
- [x] 1.2 抽出共享 skill 路径解析辅助函数，并把默认路径切换到 `.openjarvis/skills`

## 2. CLI 安装入口

- [x] 2.1 新增顶层 CLI subcommand registry，把 `skill` / `internal-mcp` / `internal-browser` 接入统一注册分发
- [x] 2.2 扩展 `src/cli.rs` 与对应 executor，新增公开命令 `openjarvis skill install <name>`
- [x] 2.3 实现首版 curated skill 安装器，支持安装 `acpx` 到工作区 `.openjarvis/skills/<name>/SKILL.md`
- [x] 2.4 为安装过程补充关键日志与清晰错误信息
- [x] 2.5 补充 `openjarvis skill uninstall <name>`，删除工作区本地 skill 目录

## 3. 测试与文档

- [x] 3.1 更新 skill registry / runtime 相关 UT，覆盖默认路径迁移后的行为
- [x] 3.2 新增 CLI registry、安装器与 CLI UT，覆盖注册分发、`acpx` 安装、覆盖写入、未知 skill 和安装后可发现行为
- [x] 3.3 更新 README 中的 skill 使用说明，补充 `openjarvis skill install acpx` 示例
- [x] 3.4 增加本地资源 fixture 与卸载、非法子命令、thread skill system message 相关 UT
- [x] 3.5 补充 `.openjarvis/skills` 的忽略规则，并同步 model/arch 文档中的默认 skill 路径
