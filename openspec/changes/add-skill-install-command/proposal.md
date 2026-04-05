## Why

当前仓库的 skill 运行时实现仍然默认扫描工作区根下的 `.skills`，这和仓库已有的 skill 架构草案不一致，也不利于把“可安装的本地 skill”与仓库源码分离管理。与此同时，用户已经明确希望把 `acpx` 这种操作手册型 skill 作为低成本方案接入，但目前没有一个稳定的本机安装入口。

如果继续依赖手工复制 `SKILL.md` 到不稳定目录，会带来几个问题：

- skill 默认目录与设计文档不一致，使用方很难知道应该放哪里
- 无法通过统一命令在本机安装 curated skill
- `acpx` 这类外部 skill 无法形成稳定验证链路

因此本次变更需要补齐本地 skill 管理的最小闭环：统一默认根目录为工作区 `.openjarvis/skills`，并新增 `openjarvis skill install <name>` 命令，首版支持安装 `acpx`。

## What Changes

- 新增用户可见的 CLI 命令空间 `openjarvis skill ...`。
- 新增统一的顶层 CLI subcommand 注册分发层，让 `openjarvis <subcommand>` 通过注册执行器运行，而不是在 `main` 里手写分支。
- 首版新增 `openjarvis skill install <name>` 子命令，支持在当前工作区本地安装 curated skill。
- 将默认本地 skill 根目录从 `.skills` 迁移到 `.openjarvis/skills`。
- 为首版 curated registry 增加 `acpx`，安装时从固定远端地址获取 `SKILL.md` 并写入本地目录。
- 增加 skill 安装、默认目录解析、启动期 `--load-skill` 和运行时扫描的测试覆盖。

## Capabilities

### New Capabilities
- `local-skill-management`: 提供工作区级 skill 安装入口和统一的默认 skill 根目录。
- `cli-subcommand-dispatch`: 提供顶层 CLI subcommand 的注册与统一分发。

### Modified Capabilities
- `skill`: 默认 skill 发现路径切换为工作区 `.openjarvis/skills`。

## Impact

- Affected code: `src/cli.rs`、`src/main.rs`、新增 `src/skill/**`、新增 `src/cli_command/**` 或等价模块、`src/agent/runtime.rs`、`src/agent/tool/mod.rs`、`src/agent/tool/skill/registry.rs` 及对应测试。
- Runtime impact: 默认 skill 扫描路径从 `.skills` 切换为 `.openjarvis/skills`。
- CLI impact: 新增公开命令 `openjarvis skill install <name>`。
- CLI structure impact: 顶层 subcommand 的执行入口从 `main` 内部手写分支切换为注册分发。
- Network impact: 安装 curated skill 时会发起一次远端 HTTP 请求下载 `SKILL.md`。
- Verification impact: 需要新增安装器与默认 skill 根目录测试，并验证 `acpx` 安装后能被 skill registry 发现。
