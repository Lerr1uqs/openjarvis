use chrono::Utc;
use openjarvis::{
    compact::{CompactRuntimeManager, CompactScopeKey},
    model::{IncomingMessage, ReplyTarget},
    session::ThreadLocator,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn compact_runtime_manager_tracks_thread_scoped_auto_compact_overrides() {
    // 测试场景: compact_enabled / auto_compact runtime override 都应只作用于对应 channel/user/external_thread_id 范围。
    let manager = CompactRuntimeManager::new();
    let target_scope = CompactScopeKey::new("feishu", "ou_a", "thread_a");
    let other_scope = CompactScopeKey::new("feishu", "ou_a", "thread_b");
    manager
        .set_compact_enabled(target_scope.clone(), true)
        .await;
    manager.set_auto_compact(target_scope.clone(), true).await;

    assert!(
        manager.compact_enabled(&target_scope, false).await,
        "target scope should read the explicit compact-enabled override"
    );
    assert!(
        manager.auto_compact_enabled(&target_scope, false).await,
        "target scope should read the explicit override"
    );
    assert!(
        !manager.compact_enabled(&other_scope, false).await,
        "other scope should keep the compact-enabled default value"
    );
    assert!(
        !manager.auto_compact_enabled(&other_scope, false).await,
        "other scope should keep the default value"
    );
}

#[test]
fn compact_scope_key_can_be_built_from_incoming_and_locator() {
    // 测试场景: 命令层和 session/agent 层应能产出相同的线程范围 key。
    let incoming = IncomingMessage {
        id: Uuid::new_v4(),
        external_message_id: Some("msg_1".to_string()),
        channel: "feishu".to_string(),
        user_id: "ou_xxx".to_string(),
        user_name: None,
        content: "hello".to_string(),
        external_thread_id: Some("thread_ext".to_string()),
        received_at: Utc::now(),
        metadata: json!({}),
        attachments: Vec::new(),
        reply_target: ReplyTarget {
            receive_id: "oc_xxx".to_string(),
            receive_id_type: "chat_id".to_string(),
        },
    };
    let locator = ThreadLocator::new(Uuid::new_v4(), &incoming, "thread_ext", Uuid::new_v4());

    assert_eq!(
        CompactScopeKey::from_incoming(&incoming),
        CompactScopeKey::from_locator(&locator)
    );
}
