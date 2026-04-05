## Context

当前 skill 体系已经具备两阶段能力：

- 启动期或运行时扫描本地 `SKILL.md`
- 通过 `load_skill` 把命中的 skill 正文和引用文件按需加载到上下文

但它还缺少“本地 skill 生命周期”的前半段，也就是：

- 默认应该去哪里找 skill
- 如何通过统一命令把一个可信 skill 安装到本地

用户当前的真实目标不是一次性做完远端 marketplace，而是先让 `acpx` 这种已有外部 skill 能稳定安装到本机，然后被 OpenJarvis 发现和使用。

这次设计遵循三个约束：

- skill 仍然是“操作手册”，不是动作执行器；执行层继续由 builtin tools 承担
- 默认 skill 根目录必须落在 `.openjarvis/skills`
- 首版安装流程先收敛为 curated registry，不扩展成任意远端 marketplace

## Goals / Non-Goals

**Goals:**

- 为顶层 `openjarvis <subcommand>` 提供一个统一的注册分发层。
- 提供公开 CLI `openjarvis skill install <name>`。
- 让默认 skill 根目录稳定切换到工作区 `.openjarvis/skills`。
- 首版支持安装 `acpx` curated skill。
- 安装后 skill registry 和 `--load-skill` 启动路径都能直接发现新 skill。

**Non-Goals:**

- 本次不做完整远端 skill marketplace。
- 本次不做 skill update / uninstall / search / enable / disable CLI。
- 本次不把 skill 存储扩展到多个默认根目录优先级策略。
- 本次不修改 `model/**` 组件文档定义。

## Decisions

### 1. 默认 skill 根目录切换为工作区 `.openjarvis/skills`

运行时默认 skill 根目录统一收敛为：

```text
<workspace>/.openjarvis/skills
```

这里的 `<workspace>` 由当前进程工作目录确定。这样和当前 memory 等本地运行时数据的目录风格保持一致，也满足用户“skill 放在本地 `.openjarvis/skills`”的要求。

Alternative considered:

- 继续使用 `.skills`
  Rejected，因为与当前架构草案不一致，也容易把运行时安装产物和仓库源码级资源混在一起。

- 默认同时扫描 `~/.openjarvis/skills` 与 `./.openjarvis/skills`
  Rejected，因为当前实现的重复 skill 优先级策略还不明确，贸然引入多根目录会扩大范围。

### 2. `openjarvis skill install` 作为独立 CLI 路径，在主程序启动前执行

`skill install` 是本地管理动作，不属于 channel/router/agent 主链路，因此应像现有 internal helper 一样，在主程序装配前短路执行并退出。

首版命令形态：

```bash
openjarvis skill install acpx
```

执行结果：

- 创建工作区 `.openjarvis/skills/acpx/`
- 下载 curated skill 的 `SKILL.md`
- 校验 frontmatter
- 原子写入本地
- 输出安装路径

Alternative considered:

- 把 skill install 做成 agent 可调用工具
  Rejected，因为本地安装属于运行时管理面，不适合作为模型直接可调用动作。

### 3. 顶层 CLI subcommand 通过注册执行器统一分发

当前 `main` 里直接根据 `cli.internal_mcp_command()`、`cli.internal_browser_command()` 等方法手写短路分支。随着公开 subcommand 增长，这种方式会持续膨胀，并且让每个 CLI 功能的执行逻辑散落在 `main`。

本次引入一个轻量 CLI command registry：

- `OpenJarvisCommand` 仍然作为 clap 解析后的顶层枚举
- 额外新增一个“执行器注册表”模块
- 每个顶层 subcommand 由单独 executor 负责执行
- `main` 只做一次 `dispatch`，若命中子命令则直接返回

首版纳入 registry 的顶层 subcommand：

- `skill`
- `internal-mcp`
- `internal-browser`

这样后续新增公开 subcommand 或内部 helper 时，只需要注册一个新的 executor，而不用继续向 `main` 追加分支。

Alternative considered:

- 继续在 `main` 里用 `if let` / `match` 分发
  Rejected，因为可维护性会随着 subcommand 数量线性下降，也不符合“集中注册管理”的目标。

### 4. 首版安装源采用 curated registry，先只内置 `acpx`

本次只支持一组程序内置的 curated skill 映射，而不是任意 URL 输入。这样有三个好处：

- scope 小，便于先把路径和安装流程打通
- 可以对远端源做最小可信约束
- UT 不需要引入复杂的远端发现协议

首版 curated 项：

- `acpx` -> `https://raw.githubusercontent.com/openclaw/acpx/main/skills/acpx/SKILL.md`

后续如果需要扩展，可以在同一 registry 里继续加别名或升级成 marketplace 文件。

Alternative considered:

- 直接支持任意 URL 安装
  Rejected，因为这会引入命名冲突、信任边界和引用文件递归下载问题，超出本次范围。

### 5. 安装流程使用“下载到内存 -> 临时文件校验 -> 原子替换”

为避免把损坏内容直接落盘，安装流程采用：

1. 下载远端 `SKILL.md`
2. 写入目标目录下临时文件
3. 用现有 skill manifest 解析逻辑校验 frontmatter
4. 校验通过后原子替换为 `SKILL.md`

若目标 skill 已存在，默认直接覆盖，以便重复安装时获取最新内容；日志中要明确记录覆盖行为。

Alternative considered:

- 目标存在时直接报错
  Rejected，因为首版缺少 `update` 命令，重复 install 作为幂等刷新更实用。

### 6. 安装器与 skill registry 共享同一套路径解析辅助函数

路径解析不能散落在 CLI、runtime、测试里各写一份。应抽出一个独立模块，统一提供：

- 默认工作区 skill 根目录解析
- 安装目标目录解析

这样 `ToolRegistry`、`AgentRuntime`、CLI 安装器和测试夹具都可以复用，减少路径漂移。

## Risks / Trade-offs

- [Curated skill 远端源不可达会导致安装失败] -> 通过清晰错误和日志暴露下载失败，不影响主程序默认启动。
- [只支持 workspace 根目录会限制跨项目复用] -> 这是有意收敛范围，后续再扩展多根目录。
- [重复安装直接覆盖可能掩盖本地手改] -> 日志中明确记录覆盖行为；未来可再加 `--force` / `--no-clobber`。
- [安装器只下载 `SKILL.md`，若未来 skill 依赖多个附属文件会不够] -> 当前 `acpx` skill 是单文件；后续再扩展引用文件下载协议。

## Migration Plan

1. 抽出共享 skill 路径解析辅助函数。
2. 将 runtime 默认 skill 根目录从 `.skills` 切换到 `.openjarvis/skills`。
3. 新增 CLI subcommand registry，并把 `skill` / `internal-mcp` / `internal-browser` 接入统一分发。
4. 扩展 CLI 和 skill executor，新增 `skill install` 命令并在主程序启动前处理。
5. 实现 curated `acpx` 安装器。
6. 更新 UT 和 README 中的使用说明。

Rollback strategy:

- 删除 `skill install` CLI 入口和安装器模块。
- 将默认 skill 根目录切回 `.skills`。
- 不影响现有 `load_skill` / `ToolRegistry` 主体机制。

## Open Questions

- 后续是否要支持 `skill install <url> --name <name>` 自定义来源。
- 后续是否要增加 `skill list` / `skill uninstall` / `skill update`。
