//! Minimal MCP registry types used to track configured MCP servers.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDefinition {
    pub name: String,
    pub transport: McpTransport,
    pub endpoint: String,
}

#[derive(Default)]
pub struct McpRegistry {
    servers: RwLock<HashMap<String, McpServerDefinition>>,
}

impl McpRegistry {
    /// Create an empty MCP registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one MCP server definition.
    pub async fn register(&self, server: McpServerDefinition) -> Result<()> {
        let mut servers = self.servers.write().await;
        if servers.contains_key(&server.name) {
            bail!("mcp server `{}` is already registered", server.name);
        }

        servers.insert(server.name.clone(), server);
        Ok(())
    }

    /// Look up one MCP server by name.
    pub async fn get(&self, name: &str) -> Option<McpServerDefinition> {
        self.servers.read().await.get(name).cloned()
    }

    /// Return all registered MCP server definitions.
    pub async fn list(&self) -> Vec<McpServerDefinition> {
        self.servers.read().await.values().cloned().collect()
    }
}
