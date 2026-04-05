use anyhow::Result;
use async_trait::async_trait;
use clap::Parser;
use openjarvis::{
    cli::{OpenJarvisCli, OpenJarvisCommand},
    cli_command::{CliCommandExecutor, CliCommandRegistry},
};
use std::sync::{Arc, Mutex};

struct RecordingCliCommandExecutor {
    command_name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CliCommandExecutor for RecordingCliCommandExecutor {
    fn name(&self) -> &'static str {
        self.command_name
    }

    async fn run(&self, command: &OpenJarvisCommand) -> Result<()> {
        self.events
            .lock()
            .expect("events lock should succeed")
            .push(command.name().to_string());
        Ok(())
    }
}

#[tokio::test]
async fn cli_command_registry_returns_false_without_top_level_subcommand() {
    // 测试场景: 没有顶层 subcommand 时，不应误触发任何 CLI executor。
    let cli = OpenJarvisCli::parse_from(["openjarvis"]);
    let registry = CliCommandRegistry::new();

    assert!(
        !registry
            .dispatch_from_cli(&cli)
            .await
            .expect("dispatch without command should succeed")
    );
}

#[tokio::test]
async fn cli_command_registry_dispatches_registered_executor_by_command_name() {
    // 测试场景: 顶层 subcommand 应通过注册名找到对应 executor，而不是在 main 中硬编码逻辑。
    let cli = OpenJarvisCli::parse_from(["openjarvis", "skill", "install", "acpx"]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut registry = CliCommandRegistry::new();
    registry
        .register(Arc::new(RecordingCliCommandExecutor {
            command_name: "skill",
            events: Arc::clone(&events),
        }))
        .expect("skill executor should register");

    assert!(
        registry
            .dispatch_from_cli(&cli)
            .await
            .expect("dispatch should succeed")
    );
    assert_eq!(
        events
            .lock()
            .expect("events lock should succeed")
            .as_slice(),
        ["skill"]
    );
}

#[test]
fn cli_command_registry_rejects_duplicate_executor_registration() {
    // 测试场景: 同名顶层 subcommand executor 只能注册一次，避免分发歧义。
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut registry = CliCommandRegistry::new();
    registry
        .register(Arc::new(RecordingCliCommandExecutor {
            command_name: "skill",
            events: Arc::clone(&events),
        }))
        .expect("first registration should succeed");

    let error = registry
        .register(Arc::new(RecordingCliCommandExecutor {
            command_name: "skill",
            events,
        }))
        .expect_err("duplicate registration should fail");

    assert!(error.to_string().contains("already registered"));
}
