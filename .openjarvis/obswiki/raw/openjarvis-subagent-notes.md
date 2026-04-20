# OpenJarvis Subagent Notes

## 目标

给主代理提供一个独立的知识库子代理，避免把外部 vault 的管理逻辑塞进主线程。

## 约束

- child thread 初始化时要注入 vault 状态、`AGENTS.md` 正文和 `index.md`。
- 模型可见工具只保留 import/search/read/write/update 五个核心动作。
- raw 层是原始资料，不允许写回覆盖。

## 调试建议

- 先通过内部 helper 直接驱动 `obswiki` 子代理。
- 观察 system prompt 是否带上 vault 运行状态。
- 再验证 search -> read -> summarize 这一条最短链路。
