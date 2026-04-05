//! Executor for public top-level skill management subcommands.

use crate::{cli::OpenJarvisCommand, skill};
use anyhow::{Result, bail};
use async_trait::async_trait;

use super::CliCommandExecutor;

/// Executor for `openjarvis skill ...`.
pub struct SkillCliCommandExecutor;

#[async_trait]
impl CliCommandExecutor for SkillCliCommandExecutor {
    fn name(&self) -> &'static str {
        "skill"
    }

    async fn run(&self, command: &OpenJarvisCommand) -> Result<()> {
        let OpenJarvisCommand::Skill(arguments) = command else {
            bail!("skill executor received mismatched top-level command");
        };
        skill::run_cli_command(&arguments.command).await
    }
}
