use super::{MemoryWorkspaceFixture, build_thread, call_tool, list_tools};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use serde_json::json;

#[tokio::test]
async fn memory_toolset_loads_per_thread_and_keeps_search_list_structured() {
    // 测试场景: memory toolset 只有在线程加载后可见，search/list 只返回结构化候选而不返回正文。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-roundtrip");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset");

    assert!(
        registry
            .list_toolsets()
            .await
            .iter()
            .any(|entry| entry.name == "memory")
    );
    assert!(
        !list_tools(&registry, &thread_context)
            .await
            .expect("initial tool listing should succeed")
            .iter()
            .any(|definition| definition.name == "memory_get")
    );

    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "memory" }),
        },
    )
    .await
    .expect("memory toolset should load");
    let loaded_names = list_tools(&registry, &thread_context)
        .await
        .expect("loaded tool listing should succeed")
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(loaded_names.iter().any(|name| name == "memory_get"));
    assert!(loaded_names.iter().any(|name| name == "memory_search"));
    assert!(loaded_names.iter().any(|name| name == "memory_write"));
    assert!(loaded_names.iter().any(|name| name == "memory_list"));

    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_write".to_string(),
            arguments: json!({
                "path": "workflow/notion.md",
                "title": "Notion 上传工作流",
                "content": "上传到 notion 时走用户自定义模板",
                "type": "active",
                "keywords": ["notion", "上传"],
            }),
        },
    )
    .await
    .expect("active memory write should succeed");
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_write".to_string(),
            arguments: json!({
                "path": "notes/preference.md",
                "title": "用户偏好",
                "content": "用户喜欢简洁中文回答",
            }),
        },
    )
    .await
    .expect("passive memory write should succeed");

    let search_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_search".to_string(),
            arguments: json!({
                "query": "notion",
                "type": "active",
                "limit": 5,
            }),
        },
    )
    .await
    .expect("memory search should succeed");
    assert!(search_result.content.contains("\"items\""));
    assert!(search_result.content.contains("workflow/notion.md"));
    assert!(
        !search_result
            .content
            .contains("上传到 notion 时走用户自定义模板")
    );

    let list_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_list".to_string(),
            arguments: json!({
                "type": "active",
            }),
        },
    )
    .await
    .expect("memory list should succeed");
    assert!(list_result.content.contains("\"items\""));
    assert!(list_result.content.contains("workflow/notion.md"));
    assert!(
        !list_result
            .content
            .contains("上传到 notion 时走用户自定义模板")
    );

    let get_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_get".to_string(),
            arguments: json!({
                "path": "workflow/notion.md",
                "type": "active",
            }),
        },
    )
    .await
    .expect("memory get should succeed");
    assert!(
        get_result
            .content
            .contains("上传到 notion 时走用户自定义模板")
    );
    assert!(get_result.content.contains("\"keywords\""));
}

#[tokio::test]
async fn memory_toolset_rejects_invalid_active_write_and_bad_paths() {
    // 测试场景: memory toolset 必须把 active keywords 约束和路径安全约束稳定暴露出来。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-invalid");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset_invalid");
    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "memory" }),
        },
    )
    .await
    .expect("memory toolset should load");

    let missing_keywords = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_write".to_string(),
            arguments: json!({
                "path": "workflow/notion.md",
                "title": "bad",
                "content": "bad",
                "type": "active",
            }),
        },
    )
    .await
    .expect_err("active write without keywords should fail");
    assert!(
        missing_keywords
            .to_string()
            .contains("requires non-empty keywords")
    );

    let passive_keywords = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_write".to_string(),
            arguments: json!({
                "path": "notes/bad.md",
                "title": "bad",
                "content": "bad",
                "keywords": ["forbidden"],
            }),
        },
    )
    .await
    .expect_err("passive write with keywords should fail");
    assert!(
        passive_keywords
            .to_string()
            .contains("must not include keywords")
    );

    let bad_get_path = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_get".to_string(),
            arguments: json!({
                "path": "/tmp/escape.md",
                "type": "passive",
            }),
        },
    )
    .await
    .expect_err("absolute get path should fail");
    assert!(bad_get_path.to_string().contains("must be relative"));
}
