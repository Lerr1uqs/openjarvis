use anyhow::Result;
use async_trait::async_trait;
use openjarvis::{
    agent::{HookEvent, HookEventKind, HookHandler, HookRegistry},
    config::AppConfig,
};
use serde_json::json;
use std::{env::temp_dir, fs, path::Path, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

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

#[tokio::test]
async fn hook_registry_loads_script_handlers_from_config_and_executes_them() {
    let config_path = temp_dir().join(format!("openjarvis-hook-config-{}.yaml", Uuid::new_v4()));
    let output_path = temp_dir().join(format!("openjarvis-hook-output-{}.txt", Uuid::new_v4()));
    let config_yaml = serde_yaml::to_string(&serde_json::json!({
        "agent": {
            "hook": {
                "notification": build_file_write_hook_command(&output_path),
            }
        },
        "llm": {
            "provider": "mock"
        }
    }))
    .expect("config yaml should serialize");
    fs::write(&config_path, config_yaml).expect("temp config should be written");

    let config = AppConfig::from_path(&config_path).expect("config should parse");
    let registry = HookRegistry::from_config(config.agent_config().hook_config())
        .await
        .expect("hook registry should build from config");
    registry
        .emit(HookEvent {
            kind: HookEventKind::Notification,
            payload: json!({"message": "hello"}),
        })
        .await
        .expect("configured hook should run");

    let output = fs::read_to_string(&output_path).expect("hook output should exist");
    fs::remove_file(&config_path).expect("temp config should be removed");
    fs::remove_file(&output_path).expect("hook output should be removed");

    assert!(output.contains("notification"));
    assert!(output.contains("\"message\":\"hello\""));
}

fn build_file_write_hook_command(output_path: &Path) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            "powershell".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            format!(
                "Set-Content -Path '{}' -Value \"$env:OPENJARVIS_HOOK_EVENT`n$env:OPENJARVIS_HOOK_PAYLOAD\"",
                output_path.display()
            ),
        ]
    }

    #[cfg(not(windows))]
    {
        vec![
            "sh".to_string(),
            "-lc".to_string(),
            format!(
                "printf '%s\\n%s\\n' \"$OPENJARVIS_HOOK_EVENT\" \"$OPENJARVIS_HOOK_PAYLOAD\" > '{}'",
                output_path.display()
            ),
        ]
    }
}
