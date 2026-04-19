//! Executors for built-in internal helper subcommands.

use crate::{
    agent::{
        run_internal_sandbox_command,
        tool::{browser, mcp::demo},
    },
    cli::OpenJarvisCommand,
};
use anyhow::{Result, bail};
use async_trait::async_trait;

use super::CliCommandExecutor;

/// Executor for `openjarvis internal-mcp ...`.
pub struct InternalMcpCliCommandExecutor;

/// Executor for `openjarvis internal-browser ...`.
pub struct InternalBrowserCliCommandExecutor;

/// Executor for `openjarvis internal-sandbox ...`.
pub struct InternalSandboxCliCommandExecutor;

#[async_trait]
impl CliCommandExecutor for InternalMcpCliCommandExecutor {
    fn name(&self) -> &'static str {
        "internal-mcp"
    }

    async fn run(&self, command: &OpenJarvisCommand) -> Result<()> {
        let OpenJarvisCommand::InternalMcp(arguments) = command else {
            bail!("internal-mcp executor received mismatched top-level command");
        };
        demo::run_internal_demo_command(&arguments.command).await
    }
}

#[async_trait]
impl CliCommandExecutor for InternalBrowserCliCommandExecutor {
    fn name(&self) -> &'static str {
        "internal-browser"
    }

    async fn run(&self, command: &OpenJarvisCommand) -> Result<()> {
        let OpenJarvisCommand::InternalBrowser(arguments) = command else {
            bail!("internal-browser executor received mismatched top-level command");
        };
        browser::run_internal_browser_command(&arguments.command).await
    }
}

#[async_trait]
impl CliCommandExecutor for InternalSandboxCliCommandExecutor {
    fn name(&self) -> &'static str {
        "internal-sandbox"
    }

    async fn run(&self, command: &OpenJarvisCommand) -> Result<()> {
        let OpenJarvisCommand::InternalSandbox(arguments) = command else {
            bail!("internal-sandbox executor received mismatched top-level command");
        };
        run_internal_sandbox_command(&arguments.command).await
    }
}
