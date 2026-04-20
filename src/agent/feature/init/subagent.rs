//! Stable initialization helpers
//! for `Feature::Subagent`.

use crate::{
    agent::ToolRegistry,
    thread::{SubagentCatalogEntry, ThreadAgentKind},
};
use tracing::{debug, info};

const SUBAGENT_MANAGED_TOOLS: [&str; 4] = [
    "spawn_subagent",
    "send_subagent",
    "close_subagent",
    "list_subagent",
];

/// Return whether the always-visible tool
/// belongs to the subagent feature.
pub fn owns_always_visible_tool(tool_name: &str) -> bool {
    SUBAGENT_MANAGED_TOOLS.contains(&tool_name)
}

pub(crate) fn managed_tool_names() -> &'static [&'static str] {
    &SUBAGENT_MANAGED_TOOLS
}

/// Build the stable subagent usage prompt
/// from the currently available subagent catalog.
pub async fn usage(thread_id: &str, tool_registry: &ToolRegistry) -> Option<String> {
    debug!(thread_id, "starting subagent feature usage prompt build");
    let catalog = if tool_registry.subagent_tools_available().await {
        let mut available = Vec::new();
        for entry in ThreadAgentKind::available_subagent_catalog() {
            let mut all_bound_toolsets_ready = true;
            for toolset_name in entry.kind.default_bound_toolsets() {
                if !tool_registry.toolset_registered(&toolset_name).await {
                    all_bound_toolsets_ready = false;
                    break;
                }
            }
            if all_bound_toolsets_ready {
                available.push(*entry);
            }
        }
        available
    } else {
        Vec::new()
    };
    let prompt = render_prompt(&catalog);
    info!(
        thread_id,
        available_subagent_count = catalog.len(),
        "built subagent feature usage prompt"
    );
    Some(prompt)
}

fn render_prompt(catalog: &[SubagentCatalogEntry]) -> String {
    if catalog.is_empty() {
        return concat!(
            "Subagent feature 已启用。\n",
            "当前可用 subagent 数量: 0。\n",
            "当前没有可用的 subagent profile，",
            "因此不要调用 spawn_subagent、send_subagent、close_subagent 或 list_subagent。"
        )
        .to_string();
    }

    let catalog_lines = catalog
        .iter()
        .map(|entry| {
            format!(
                concat!(
                    "- subagent_key: {}\n",
                    "  role_summary: {}\n",
                    "  when_to_use: {}\n",
                    "  when_not_to_use: {}"
                ),
                entry.subagent_key, entry.role_summary, entry.when_to_use, entry.when_not_to_use,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        concat!(
            "Subagent feature 已启用。\n",
            "当前可用 subagent 数量: {}。\n",
            "可用 subagent catalog:\n{}\n",
            "使用原则:\n",
            "- `spawn_subagent` 用于启动某个 subagent，并把首个任务直接交给它执行。\n",
            "- 只有当 `persist` child thread 已存在时，才使用 `send_subagent` 做后续交互。\n",
            "- `yolo` 模式只执行一次，不需要 `send_subagent` 或 `close_subagent` 继续管理。\n",
            "- 当任务明显属于某个专用 profile，且需要独立子线程上下文时，优先使用 subagent。\n",
            "- 简单直接的工具调用不应默认升级成 subagent 调用。\n",
            "- 当某个 profile 已存在可复用 child thread 时，应优先复用，而不是额外创建并行实例。\n",
            "可用管理工具: {}"
        ),
        catalog.len(),
        catalog_lines,
        managed_tool_names().join(", "),
    )
}
