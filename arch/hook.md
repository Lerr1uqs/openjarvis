Hook 事件
Python
TypeScript
触发时机
典型用途
PreToolUse
✅
✅
工具调用前（可阻断/修改）
拦截危险命令、注入凭证
PostToolUse
✅
✅
工具执行后（有结果）
审计日志、记录文件变更
PostToolUseFailure
✅
✅
工具执行失败时
错误处理、告警通知
UserPromptSubmit
✅
✅
用户提交 prompt 时
注入上下文、内容过滤
Stop
✅
✅
Agent 执行停止时
保存状态、资源清理
SubagentStart
✅
✅
子 Agent 初始化时
追踪并行任务启动
SubagentStop
✅
✅
子 Agent 完成时
汇总并行任务结果
PreCompact
✅
✅
对话压缩/摘要前
归档完整对话记录
PermissionRequest
✅
✅
需要权限确认时
自定义审批流程
Notification
✅
✅
Agent 发送状态消息时
推送通知到 Slack/PagerDuty
SessionStart
❌
✅
会话初始化时
初始化日志/监控
SessionEnd
❌
✅
会话结束时
清理临时资源
Setup
❌
✅
会话设置/维护阶段
执行初始化任务
TeammateIdle
❌
✅
协作者空闲时
重新分配任务
TaskCompleted
❌
✅
后台任务完成时
聚合任务结果
ConfigChange
❌
✅
配置文件变更时
动态重载配置
WorktreeCreate
❌
✅
Git worktree 创建时
追踪隔离工作区
WorktreeRemove
❌
✅
Git worktree 删除时
清理工作区资源