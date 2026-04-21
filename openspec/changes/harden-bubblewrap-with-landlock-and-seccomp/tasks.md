## 1. Capability Policy 与 enforcement plan

- [x] 1.1 扩展 `config/capabilities.yaml` 解析结构，增加 bubblewrap 下的 namespace、baseline seccomp、proxy landlock、command profile 与兼容性字段
- [x] 1.2 为新增 capability policy 字段补充校验逻辑，覆盖未知 profile、空白配置、最小 Landlock ABI 与 seccomp 要求
- [x] 1.3 在 `src/agent/sandbox.rs` 中实现 `SandboxKernelEnforcementPlan` 及其编译流程，统一产出 namespace/mount、proxy、child 三层 enforcement 计划

## 2. Bubblewrap runtime 与 proxy 启动收口

- [x] 2.1 扩展 `configure_bubblewrap_command`，根据 enforcement plan 生成 namespace 开关和 baseline seccomp 安装输入
- [x] 2.2 扩展 `internal-sandbox proxy` 启动协议，使宿主侧能把结构化 enforcement plan 传入 proxy
- [x] 2.3 在 proxy 启动路径中实现 `no_new_privs` 与 proxy 级 Landlock 安装，并在失败时显式中止握手
- [x] 2.4 为 bubblewrap backend 增加 fail-fast 错误路径，覆盖 plan 不可满足、proxy 启动失败和 enforcement 安装失败

## 3. Command child enforcement

- [x] 3.1 新增 `internal-sandbox exec` 隐藏 helper，用于在真正执行用户命令前安装 child 级 Landlock 与 Seccomp
- [x] 3.2 调整 sandbox 内 `exec_command` 路径，让 pipe/PTY 两条命令执行链统一通过 child helper 启动
- [x] 3.3 将 command profile 选择接入 command session/runtime，确保未知 profile、profile 拒绝和内核不支持时都显式报错

## 4. 测试与文档

- [x] 4.1 为 capability policy 增加配置测试，覆盖 enforcement profile、兼容性字段和 fail-closed 行为
- [x] 4.2 为 sandbox 集成测试增加 proxy 启动收口、child enforcement 拒绝和 baseline seccomp 失败路径
- [x] 4.3 为 command session 增加 child helper 回归测试，覆盖 pipe/PTY 两条路径的 profile 安装
- [x] 4.4 更新 README / 本地依赖说明，补充 Landlock、Seccomp、bubblewrap 相关运行前提与调试方式
