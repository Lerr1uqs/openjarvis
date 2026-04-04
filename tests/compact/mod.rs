mod budget;
mod manager;
mod provider;

use openjarvis::compact::{CompactManager, CompactSummary, StaticCompactProvider};
use std::sync::Arc;

#[test]
fn compact_module_reexports_can_build_manager() {
    // 测试场景: compact 模块对外暴露的核心类型可以直接组合使用，方便 runtime 按消息边界接入。
    let _manager = CompactManager::new(Arc::new(StaticCompactProvider::new(CompactSummary {
        compacted_assistant: "压缩后的上下文".to_string(),
    })));
}
