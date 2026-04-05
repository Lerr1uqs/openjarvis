use super::MemoryWorkspaceFixture;
use openjarvis::agent::memory::{MemoryRepository, MemoryType, MemoryWriteRequest};
use std::fs;

#[test]
fn memory_repository_roundtrips_active_and_passive_documents() {
    // 测试场景: repository 要能把 active/passive markdown memory 写入文件系统并再次解析回来。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-repository-roundtrip");
    let repository = MemoryRepository::new(fixture.root());

    let passive = repository
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Passive,
            path: "notes/user-preference.md".to_string(),
            title: "用户偏好".to_string(),
            content: "用户喜欢简洁中文回答".to_string(),
            keywords: None,
        })
        .expect("passive memory should write");
    let active = repository
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "workflow/notion.md".to_string(),
            title: "Notion 上传工作流".to_string(),
            content: "上传到 notion 时走用户自定义模板".to_string(),
            keywords: Some(vec!["notion".to_string(), "上传".to_string()]),
        })
        .expect("active memory should write");

    let passive_file = fixture
        .memory_root()
        .join("passive/notes/user-preference.md");
    let active_file = fixture.memory_root().join("active/workflow/notion.md");
    assert!(passive_file.exists());
    assert!(active_file.exists());

    let passive_loaded = repository
        .get(MemoryType::Passive, "notes/user-preference.md")
        .expect("passive memory should load");
    let active_loaded = repository
        .get(MemoryType::Active, "workflow/notion.md")
        .expect("active memory should load");

    assert_eq!(passive_loaded.content, passive.content);
    assert_eq!(active_loaded.content, active.content);
    assert_eq!(
        active_loaded.metadata.keywords,
        vec!["notion".to_string(), "上传".to_string()]
    );

    let catalog = repository
        .load_active_catalog()
        .expect("active catalog should build");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].path, "workflow/notion.md");
    assert_eq!(
        catalog[0].keywords,
        vec!["notion".to_string(), "上传".to_string()]
    );
    assert!(
        fs::read_to_string(active_file)
            .expect("active memory file should exist")
            .contains("keywords:")
    );
    assert!(
        !fs::read_to_string(passive_file)
            .expect("passive memory file should exist")
            .contains("keywords:")
    );
}

#[test]
fn memory_repository_active_catalog_prompt_groups_multiple_keywords_per_file() {
    // 测试场景: 同一个 active memory 文件声明多个关键词时，catalog prompt 必须合并成一行，
    // 不能错误地展开成多条 `keyword -> path` 映射。
    let fixture =
        MemoryWorkspaceFixture::new("openjarvis-memory-repository-grouped-active-catalog-prompt");
    let repository = MemoryRepository::new(fixture.root());

    repository
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Active,
            path: "profile/jjj_preference.md".to_string(),
            title: "JJJ 偏好".to_string(),
            content: "JJJ 喜欢被叫小南梁".to_string(),
            keywords: Some(vec![
                "JJJ".to_string(),
                "喜好".to_string(),
                "小南梁".to_string(),
            ]),
        })
        .expect("grouped active memory should write");

    let prompt = repository
        .active_catalog_prompt()
        .expect("active catalog prompt should build")
        .expect("active catalog prompt should exist");
    let prompt_lines = prompt.lines().collect::<Vec<_>>();

    assert!(prompt.contains("JJJ, 喜好, 小南梁 -> profile/jjj_preference.md"));
    assert!(
        !prompt_lines
            .iter()
            .any(|line| line.trim() == "- JJJ -> profile/jjj_preference.md")
    );
    assert!(
        !prompt_lines
            .iter()
            .any(|line| line.trim() == "- 喜好 -> profile/jjj_preference.md")
    );
    assert!(
        !prompt_lines
            .iter()
            .any(|line| line.trim() == "- 小南梁 -> profile/jjj_preference.md")
    );
}

#[test]
fn memory_repository_rejects_active_document_without_keywords() {
    // 测试场景: active memory frontmatter 缺少 keywords 时，catalog 构建必须失败。
    let fixture =
        MemoryWorkspaceFixture::new("openjarvis-memory-repository-active-missing-keywords");
    let repository = MemoryRepository::new(fixture.root());
    let active_root = fixture.memory_root().join("active/workflow");
    fs::create_dir_all(&active_root).expect("active memory directory should exist");
    fs::write(
        active_root.join("notion.md"),
        r#"---
title: "Notion 上传工作流"
created_at: 2026-04-05T10:00:00Z
updated_at: 2026-04-05T10:00:00Z
---
上传到 notion 时走用户自定义模板
"#,
    )
    .expect("invalid active memory fixture should be written");

    let error = repository
        .load_active_catalog()
        .expect_err("active memory without keywords should fail");
    assert!(error.to_string().contains("keywords"));
}

#[test]
fn memory_repository_rejects_duplicate_active_keywords() {
    // 测试场景: 两个 active 文档声明同一个 keyword 时，repository 必须拒绝构建歧义 catalog。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-repository-duplicate-keywords");
    let repository = MemoryRepository::new(fixture.root());
    let active_root = fixture.memory_root().join("active/workflow");
    fs::create_dir_all(&active_root).expect("active memory directory should exist");
    fs::write(
        active_root.join("a.md"),
        r#"---
title: "A"
created_at: 2026-04-05T10:00:00Z
updated_at: 2026-04-05T10:00:00Z
keywords:
  - notion
---
A
"#,
    )
    .expect("first active memory fixture should be written");
    fs::write(
        active_root.join("b.md"),
        r#"---
title: "B"
created_at: 2026-04-05T10:00:00Z
updated_at: 2026-04-05T10:00:00Z
keywords:
  - notion
---
B
"#,
    )
    .expect("second active memory fixture should be written");

    let error = repository
        .load_active_catalog()
        .expect_err("duplicate active keyword should fail");
    assert!(
        error
            .to_string()
            .contains("duplicate active memory keyword")
    );
}

#[test]
fn memory_repository_rejects_illegal_paths_and_passive_keywords() {
    // 测试场景: write/get 必须拒绝目录逃逸、绝对路径和 passive keywords。
    let fixture = MemoryWorkspaceFixture::new("openjarvis-memory-repository-invalid-path");
    let repository = MemoryRepository::new(fixture.root());

    let escape_error = repository
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Passive,
            path: "../escape.md".to_string(),
            title: "bad".to_string(),
            content: "bad".to_string(),
            keywords: None,
        })
        .expect_err("parent path should be rejected");
    assert!(escape_error.to_string().contains("must not contain `..`"));

    let absolute_error = repository
        .get(MemoryType::Passive, "/tmp/escape.md")
        .expect_err("absolute path should be rejected");
    assert!(absolute_error.to_string().contains("must be relative"));

    let passive_keyword_error = repository
        .write(MemoryWriteRequest {
            memory_type: MemoryType::Passive,
            path: "notes/with-keywords.md".to_string(),
            title: "bad".to_string(),
            content: "bad".to_string(),
            keywords: Some(vec!["forbidden".to_string()]),
        })
        .expect_err("passive keywords should be rejected");
    assert!(
        passive_keyword_error
            .to_string()
            .contains("must not include keywords")
    );
}
