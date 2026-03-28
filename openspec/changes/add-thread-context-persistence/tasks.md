## 1. Store Abstraction

- [ ] 1.1 新增线程持久化配置和 `SessionStore` trait，定义 session 解析、线程快照读写、去重记录和 schema 初始化接口
- [ ] 1.2 提供 memory store 兼容实现，保证现有测试和非持久化场景仍可复用同一接口
- [ ] 1.3 为 `SessionManager` 注入 store 依赖，并收口为“缓存 + store”统一读写入口

## 2. SQLite Backend

- [ ] 2.1 实现 SQLite store，增加 schema 初始化、版本迁移和数据库文件路径解析
- [ ] 2.2 定义线程快照表、session/thread 元数据表和 `external_message_id` 去重表
- [ ] 2.3 将 `ThreadContext` 快照按 turn 结构落盘，并补充 revision/CAS 写入语义

## 3. Runtime Integration

- [ ] 3.1 在 `load_or_create_thread` 和 `load_thread_context` 路径接入 cache miss 懒加载恢复
- [ ] 3.2 在 `store_thread_context`、`store_turn_with_thread_context` 等写路径接入 write-through 持久化
- [ ] 3.3 让工具集恢复、compact 兼容缓存和线程命令状态都从持久化后的 `ThreadContext` 重建，而不是从旧兼容缓存反向覆盖

## 4. Verification

- [ ] 4.1 为 store trait、SQLite store 和 `SessionManager` 恢复路径补充 UT，覆盖 session/thread 创建、turn 边界保存、revision 冲突和去重行为
- [ ] 4.2 增加重启恢复集成测试，覆盖 compact turn、loaded toolsets、`/auto-compact` 状态和同线程消息恢复
- [ ] 4.3 更新架构文档和配置文档，说明持久化边界、SQLite 默认行为以及未来 PostgreSQL 扩展方式
