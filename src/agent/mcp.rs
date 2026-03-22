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
    pub fn new() -> Self {
        // 作用: 创建一个空的 MCP 服务注册表。
        // 参数: 无，默认没有任何 MCP 服务端定义。
        Self::default()
    }

    pub async fn register(&self, server: McpServerDefinition) -> Result<()> {
        // 作用: 注册一个 MCP 服务端定义，供后续 agent loop 或工具层访问。
        // 参数: server 为 MCP 服务名称、传输方式和连接端点定义。
        let mut servers = self.servers.write().await;
        if servers.contains_key(&server.name) {
            bail!("mcp server `{}` is already registered", server.name);
        }

        servers.insert(server.name.clone(), server);
        Ok(())
    }

    pub async fn get(&self, name: &str) -> Option<McpServerDefinition> {
        // 作用: 根据服务名称查询 MCP 服务定义。
        // 参数: name 为已注册 MCP 服务的唯一名称。
        self.servers.read().await.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<McpServerDefinition> {
        // 作用: 返回当前全部已注册的 MCP 服务定义。
        // 参数: 无，结果来自内存中的 registry 状态。
        self.servers.read().await.values().cloned().collect()
    }
}
