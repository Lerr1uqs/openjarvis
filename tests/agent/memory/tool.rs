use super::{
    HybridMockServer, MemoryWorkspaceFixture, build_config_from_yaml, build_thread, call_tool,
    hybrid_config_yaml, install_fixture_as_cwd, list_tools, parse_tool_json,
    seed_hybrid_memory_corpus, unique_paths, write_memory_document,
};
use openjarvis::agent::{ToolCallRequest, ToolRegistry};
use serde_json::json;

async fn load_memory_toolset(
    registry: &ToolRegistry,
    thread_context: &mut openjarvis::thread::Thread,
) {
    call_tool(
        registry,
        thread_context,
        ToolCallRequest {
            name: "load_toolset".to_string(),
            arguments: json!({ "name": "memory" }),
        },
    )
    .await
    .expect("memory toolset should load");
}

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

#[tokio::test]
async fn memory_write_schema_requires_user_provided_specific_active_keywords() {
    // 测试场景: memory_write 的 tool schema 必须明确要求 active keywords 只能使用用户明确提供的专用名字，
    // 如果用户没说清楚则先问，不允许模型自行脑补关键词。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-schema");
    let registry = ToolRegistry::with_workspace_root_and_skill_roots(fixture.root(), Vec::new());
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset_schema");
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

    let definition = list_tools(&registry, &thread_context)
        .await
        .expect("loaded tool listing should succeed")
        .into_iter()
        .find(|definition| definition.name == "memory_write")
        .expect("memory_write definition should exist");
    let schema = definition.input_schema.json_schema();
    let keywords_description = schema["properties"]["keywords"]["description"]
        .as_str()
        .expect("keywords description should exist");

    assert!(
        definition
            .description
            .contains("Do not invent extra keywords")
    );
    assert!(definition.description.contains("ask first"));
    assert!(keywords_description.contains("highly specific names"));
    assert!(keywords_description.contains("directly provided by the user"));
    assert!(keywords_description.contains("ask first"));
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_memory_search_uses_default_models_and_returns_structured_semantic_results() {
    // 测试场景: hybrid 模式在未显式覆盖模型时要使用默认 SiliconFlow 模型，并且语义相关文档
    // 仍然只以结构化候选形式返回，不能泄露正文。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-hybrid-default-models");
    seed_hybrid_memory_corpus(fixture.root());
    let mock_server = HybridMockServer::start(&fixture).await;
    let cwd_guard = install_fixture_as_cwd(fixture.root());
    let config = build_config_from_yaml(&hybrid_config_yaml(
        &mock_server.base_url(),
        mock_server
            .api_key_path()
            .to_str()
            .expect("api key path should be utf-8"),
        "",
    ));
    let registry =
        ToolRegistry::from_config_with_skill_roots(config.agent_config().tool_config(), Vec::new())
            .await
            .expect("hybrid registry should build");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset_hybrid_default_models");
    load_memory_toolset(&registry, &mut thread_context).await;

    let result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_search".to_string(),
            arguments: json!({
                "query": "以后回答时记住我的表达方式",
                "type": "passive",
                "limit": 2,
            }),
        },
    )
    .await
    .expect("hybrid memory search should succeed");
    let payload = parse_tool_json(&result.content);
    let items = payload["items"]
        .as_array()
        .expect("hybrid memory search items should be an array");
    let returned_paths = unique_paths(items);

    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0]["path"].as_str(),
        Some("preferences/semantic-style-fresh.md")
    );
    assert!(returned_paths.contains("preferences/semantic-style-fresh.md"));
    assert!(!returned_paths.contains("preferences/noise.md"));
    assert!(!result.content.contains("默认使用中文"));
    assert!(!result.content.contains("回答保持简洁"));

    let records = mock_server.records().await;
    assert!(
        records
            .embedding_models
            .iter()
            .all(|model| model == "BAAI/bge-large-zh-v1.5")
    );
    assert!(
        records
            .rerank_models
            .iter()
            .all(|model| model == "BAAI/bge-reranker-v2-m3")
    );
    drop(cwd_guard);
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_memory_search_reuses_embedding_cache_and_refreshes_only_changed_documents() {
    // 测试场景: dense embedding cache 首次搜索会为缺失文档增量建索引，后续命中 cache，
    // 文档内容更新后只刷新变更文档，不应整库重算。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-hybrid-cache");
    seed_hybrid_memory_corpus(fixture.root());
    let mock_server = HybridMockServer::start(&fixture).await;
    let cwd_guard = install_fixture_as_cwd(fixture.root());
    let config = build_config_from_yaml(&hybrid_config_yaml(
        &mock_server.base_url(),
        mock_server
            .api_key_path()
            .to_str()
            .expect("api key path should be utf-8"),
        "",
    ));
    let registry =
        ToolRegistry::from_config_with_skill_roots(config.agent_config().tool_config(), Vec::new())
            .await
            .expect("hybrid registry should build");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset_hybrid_cache");
    load_memory_toolset(&registry, &mut thread_context).await;

    for _ in 0..2 {
        call_tool(
            &registry,
            &mut thread_context,
            ToolCallRequest {
                name: "memory_search".to_string(),
                arguments: json!({
                    "query": "以后回答时记住我的表达方式",
                    "type": "passive",
                    "limit": 2,
                }),
            },
        )
        .await
        .expect("hybrid memory search should succeed");
    }

    let records = mock_server.records().await;
    assert_eq!(records.embedding_batch_sizes[0], 1);
    assert_eq!(records.embedding_batch_sizes[1], 4);
    assert_eq!(records.embedding_batch_sizes[2], 1);
    drop(records);

    write_memory_document(
        &fixture
            .memory_root()
            .join("passive/preferences/semantic-style-fresh.md"),
        r#"---
title: "最新回答风格"
created_at: 2026-04-10T10:00:00Z
updated_at: 2026-04-19T10:00:00Z
---
最近更新：默认使用中文，回答保持非常简洁，先给结论再补背景。
"#,
    );

    call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_search".to_string(),
            arguments: json!({
                "query": "以后回答时记住我的表达方式",
                "type": "passive",
                "limit": 2,
            }),
        },
    )
    .await
    .expect("hybrid memory search should succeed after one document update");

    let records = mock_server.records().await;
    assert_eq!(records.embedding_batch_sizes[3], 1);
    assert_eq!(records.embedding_batch_sizes[4], 1);
    drop(cwd_guard);
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_memory_search_honors_type_filter_and_fails_fast_on_remote_errors() {
    // 测试场景: hybrid 模式应继续尊重 type 过滤，并且远程 provider 报错时必须显式失败，
    // 不能偷偷回退到 lexical。
    let fixture =
        MemoryWorkspaceFixture::new("openjarvis-memory-toolset-hybrid-type-filter-and-failure");
    seed_hybrid_memory_corpus(fixture.root());
    let mock_server = HybridMockServer::start(&fixture).await;
    let cwd_guard = install_fixture_as_cwd(fixture.root());
    let config = build_config_from_yaml(&hybrid_config_yaml(
        &mock_server.base_url(),
        mock_server
            .api_key_path()
            .to_str()
            .expect("api key path should be utf-8"),
        "",
    ));
    let registry =
        ToolRegistry::from_config_with_skill_roots(config.agent_config().tool_config(), Vec::new())
            .await
            .expect("hybrid registry should build");
    registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut thread_context = build_thread("thread_memory_toolset_hybrid_type_filter");
    load_memory_toolset(&registry, &mut thread_context).await;

    let active_result = call_tool(
        &registry,
        &mut thread_context,
        ToolCallRequest {
            name: "memory_search".to_string(),
            arguments: json!({
                "query": "notion 上传模板",
                "type": "active",
                "limit": 5,
            }),
        },
    )
    .await
    .expect("hybrid active search should succeed");
    let active_payload = parse_tool_json(&active_result.content);
    let active_items = active_payload["items"]
        .as_array()
        .expect("active items should be an array");
    assert_eq!(active_items.len(), 1);
    assert_eq!(active_items[0]["type"].as_str(), Some("active"));
    assert_eq!(active_items[0]["path"].as_str(), Some("workflow/notion.md"));
    drop(cwd_guard);

    let failing_fixture = MemoryWorkspaceFixture::new("openjarvis-memory-toolset-hybrid-failure");
    seed_hybrid_memory_corpus(failing_fixture.root());
    let failing_server = HybridMockServer::start_with_options(&failing_fixture, false, true).await;
    let failing_cwd_guard = install_fixture_as_cwd(failing_fixture.root());
    let failing_config = build_config_from_yaml(&hybrid_config_yaml(
        &failing_server.base_url(),
        failing_server
            .api_key_path()
            .to_str()
            .expect("api key path should be utf-8"),
        "",
    ));
    let failing_registry = ToolRegistry::from_config_with_skill_roots(
        failing_config.agent_config().tool_config(),
        Vec::new(),
    )
    .await
    .expect("failing hybrid registry should build");
    failing_registry
        .register_builtin_tools()
        .await
        .expect("builtin tools should register");
    let mut failing_thread_context = build_thread("thread_memory_toolset_hybrid_failure");
    load_memory_toolset(&failing_registry, &mut failing_thread_context).await;

    let error = call_tool(
        &failing_registry,
        &mut failing_thread_context,
        ToolCallRequest {
            name: "memory_search".to_string(),
            arguments: json!({
                "query": "notion 上传模板",
                "type": "active",
                "limit": 5,
            }),
        },
    )
    .await
    .expect_err("hybrid search should fail when rerank provider errors");
    let error_text = error.to_string().to_ascii_lowercase();
    assert!(error_text.contains("rerank"));
    assert!(error_text.contains("failed"));

    drop(failing_cwd_guard);
}
