mod budget;
mod manager;
mod provider;
mod runtime;
mod strategy;

use chrono::Utc;
use openjarvis::compact::{
    CompactAllChatStrategy, CompactManager, CompactSummary, StaticCompactProvider,
};
use std::sync::Arc;

#[test]
fn compact_module_reexports_can_build_manager() {
    // 测试场景: compact 模块对外暴露的核心类型可以直接组合使用，方便后续接入 runtime。
    let _now = Utc::now();
    let _manager = CompactManager::new(
        Arc::new(StaticCompactProvider::new(CompactSummary {
            compacted_assistant: "压缩后的上下文".to_string(),
        })),
        Arc::new(CompactAllChatStrategy),
    );
}
