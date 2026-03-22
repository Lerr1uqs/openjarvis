use anyhow::Result;
use async_trait::async_trait;
use openjarvis::agent::{HookEvent, HookEventKind, HookHandler, HookRegistry};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

struct RecordingHook {
    seen: Arc<Mutex<Vec<HookEventKind>>>,
}

#[async_trait]
impl HookHandler for RecordingHook {
    fn name(&self) -> &'static str {
        "recording_hook"
    }

    async fn handle(&self, event: &HookEvent) -> Result<()> {
        self.seen.lock().await.push(event.kind.clone());
        Ok(())
    }
}

#[tokio::test]
async fn hook_registry_emits_events_to_registered_handlers() {
    let registry = HookRegistry::new();
    let seen = Arc::new(Mutex::new(Vec::new()));

    registry
        .register(Arc::new(RecordingHook {
            seen: Arc::clone(&seen),
        }))
        .await;
    registry
        .emit(HookEvent {
            kind: HookEventKind::Notification,
            payload: json!({"message": "hello"}),
        })
        .await
        .expect("hook emit should succeed");

    let events = seen.lock().await;
    assert_eq!(events.as_slice(), &[HookEventKind::Notification]);
}
