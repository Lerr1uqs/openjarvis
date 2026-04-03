use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use openjarvis::{
    agent::{
        AgentRuntime, ToolCallRequest, ToolCallResult, ToolDefinition, ToolHandler, ToolRegistry,
        ToolSource, ToolsetCatalogEntry, empty_tool_input_schema,
    },
    config::AppConfig,
    thread::{ThreadContext, ThreadContextLocator},
};
use serde_json::json;
use std::sync::Arc;

use super::tool::mcp::demo_stdio_config;

struct DemoRuntimeTool;

#[async_trait]
impl ToolHandler for DemoRuntimeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "demo_runtime__echo".to_string(),
            description: "Echo from runtime tests".to_string(),
            input_schema: empty_tool_input_schema(),
            source: ToolSource::Builtin,
        }
    }

    async fn call(&self, _request: ToolCallRequest) -> Result<ToolCallResult> {
        Ok(ToolCallResult {
            content: "runtime-demo".to_string(),
            metadata: json!({}),
            is_error: false,
        })
    }
}

#[tokio::test]
async fn default_runtime_starts_with_empty_registries() {
    let runtime = AgentRuntime::new();

    assert_eq!(runtime.hooks().len().await, 0);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.tools().mcp().list_servers().await.len(), 0);
}

#[tokio::test]
async fn runtime_from_config_loads_configured_hooks() {
    let config: AppConfig = serde_yaml::from_str(
        r#"
agent:
  hook:
    notification: ["echo", "hello"]
llm:
  provider: "mock"
"#,
    )
    .expect("config should parse");
    let runtime = AgentRuntime::from_config_with_skill_roots(config.agent_config(), Vec::new())
        .await
        .expect("runtime should build");

    assert_eq!(runtime.hooks().len().await, 1);
    assert_eq!(runtime.tools().list().await.len(), 0);
    assert_eq!(runtime.tools().mcp().list_servers().await.len(), 0);
}

#[tokio::test]
async fn runtime_from_config_loads_tool_managed_mcp_servers() {
    let config = demo_stdio_config(false);
    let runtime = AgentRuntime::from_config_with_skill_roots(config.agent_config(), Vec::new())
        .await
        .expect("runtime should build");

    let servers = runtime.tools().mcp().list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "demo_stdio");
    assert!(!servers[0].enabled);
    assert_eq!(servers[0].tool_count, 0);
}

#[tokio::test]
async fn runtime_manages_thread_tool_visibility_and_open_close_flow() {
    // 测试场景: runtime 应负责当前线程的工具可见性，并暴露 open_tool / close_tool / list_tools。
    let registry = Arc::new(ToolRegistry::with_skill_roots(Vec::new()));
    registry
        .register_toolset(
            ToolsetCatalogEntry::new("demo_runtime", "Demo runtime toolset"),
            vec![Arc::new(DemoRuntimeTool)],
        )
        .await
        .expect("demo runtime toolset should register");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let runtime = AgentRuntime::with_parts(Default::default(), registry);
    let mut thread_context = ThreadContext::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_runtime", "thread_runtime"),
        Utc::now(),
    );

    let initial_tools = runtime
        .list_tools(&thread_context, false)
        .await
        .expect("runtime should list baseline tools");
    assert!(!initial_tools.iter().any(|tool| tool.name == "compact"));
    assert!(
        !initial_tools
            .iter()
            .any(|tool| tool.name == "demo_runtime__echo")
    );

    assert!(
        runtime
            .open_tool(&mut thread_context, "demo_runtime")
            .await
            .expect("runtime should open demo tool")
    );
    let opened_tools = runtime
        .list_tools(&thread_context, true)
        .await
        .expect("runtime should list tools after opening");
    assert!(opened_tools.iter().any(|tool| tool.name == "compact"));
    assert!(
        opened_tools
            .iter()
            .any(|tool| tool.name == "demo_runtime__echo")
    );

    assert!(
        runtime
            .close_tool(&mut thread_context, "demo_runtime")
            .await
            .expect("runtime should close demo tool")
    );
    let closed_tools = runtime
        .list_tools(&thread_context, false)
        .await
        .expect("runtime should list tools after closing");
    assert!(
        !closed_tools
            .iter()
            .any(|tool| tool.name == "demo_runtime__echo")
    );
}
