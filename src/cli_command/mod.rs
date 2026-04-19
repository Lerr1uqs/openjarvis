//! Registered top-level CLI subcommand dispatch for the OpenJarvis binary.

pub mod internal;
pub mod skill;

use crate::cli::{OpenJarvisCli, OpenJarvisCommand};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use tracing::info;

/// One registered executor for a top-level `openjarvis <subcommand>`.
#[async_trait]
pub trait CliCommandExecutor: Send + Sync {
    /// Return the stable top-level subcommand name handled by this executor.
    fn name(&self) -> &'static str;

    /// Execute the provided parsed top-level subcommand.
    async fn run(&self, command: &OpenJarvisCommand) -> Result<()>;
}

/// Registry of top-level CLI subcommand executors.
pub struct CliCommandRegistry {
    executors: BTreeMap<String, Arc<dyn CliCommandExecutor>>,
}

impl CliCommandRegistry {
    /// Create an empty CLI command registry.
    pub fn new() -> Self {
        Self {
            executors: BTreeMap::new(),
        }
    }

    /// Create a CLI command registry with the built-in OpenJarvis executors.
    pub fn with_builtin_commands() -> Result<Self> {
        let mut registry = Self::new();
        registry.register(Arc::new(skill::SkillCliCommandExecutor))?;
        registry.register(Arc::new(internal::InternalMcpCliCommandExecutor))?;
        registry.register(Arc::new(internal::InternalBrowserCliCommandExecutor))?;
        registry.register(Arc::new(internal::InternalSandboxCliCommandExecutor))?;
        Ok(registry)
    }

    /// Register one executor by its stable subcommand name.
    pub fn register(&mut self, executor: Arc<dyn CliCommandExecutor>) -> Result<()> {
        let command_name = executor.name().trim();
        if command_name.is_empty() {
            bail!("cli command executor name must not be blank");
        }
        if self.executors.contains_key(command_name) {
            bail!("cli command executor `{command_name}` is already registered");
        }
        self.executors.insert(command_name.to_string(), executor);
        Ok(())
    }

    /// Dispatch the parsed CLI if it contains a top-level subcommand.
    ///
    /// Returns `true` when a registered subcommand was executed.
    pub async fn dispatch_from_cli(&self, cli: &OpenJarvisCli) -> Result<bool> {
        let Some(command) = cli.command.as_ref() else {
            return Ok(false);
        };

        self.dispatch(command).await?;
        Ok(true)
    }

    /// Dispatch one parsed top-level subcommand to its registered executor.
    pub async fn dispatch(&self, command: &OpenJarvisCommand) -> Result<()> {
        let command_name = command.name();
        let executor = self
            .executors
            .get(command_name)
            .with_context(|| format!("no cli command executor registered for `{command_name}`"))?;
        info!(command_name, "dispatching top-level cli subcommand");
        executor.run(command).await
    }
}

impl Default for CliCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
