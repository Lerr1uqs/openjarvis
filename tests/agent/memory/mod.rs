mod feature;
mod repository;
mod tool;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use openjarvis::{
    agent::{ToolCallRequest, ToolCallResult, ToolDefinition, ToolRegistry},
    config::AppConfig,
    thread::{Thread, ThreadContextLocator},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    env::temp_dir,
    env::{current_dir, set_current_dir},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex, task::JoinHandle};
use uuid::Uuid;

pub(crate) struct MemoryWorkspaceFixture {
    root: PathBuf,
}

impl MemoryWorkspaceFixture {
    pub(crate) fn new(prefix: &str) -> Self {
        let root = temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("memory workspace root should be created");
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn memory_root(&self) -> PathBuf {
        self.root.join(".openjarvis/memory")
    }
}

impl Drop for MemoryWorkspaceFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn build_thread(thread_id: &str) -> Thread {
    Thread::new(
        ThreadContextLocator::new(None, "feishu", "ou_xxx", thread_id, thread_id),
        chrono::Utc::now(),
    )
}

pub(crate) async fn list_tools(
    registry: &ToolRegistry,
    thread_context: &Thread,
) -> anyhow::Result<Vec<ToolDefinition>> {
    registry.list_for_context(thread_context).await
}

pub(crate) async fn call_tool(
    registry: &ToolRegistry,
    thread_context: &mut Thread,
    request: ToolCallRequest,
) -> anyhow::Result<ToolCallResult> {
    registry.call_for_context(thread_context, request).await
}

pub(crate) fn install_fixture_as_cwd(path: &Path) -> ScopedCurrentDir {
    ScopedCurrentDir::enter(path)
}

pub(crate) fn hybrid_config_yaml(base_url: &str, api_key_path: &str, extra: &str) -> String {
    format!(
        r#"
agent:
  tool:
    memory:
      search:
        mode: "hybrid"
        hybrid:
          base_url: "{base_url}"
          api_key_path: "{api_key_path}"
{extra}
llm:
  protocol: "mock"
  provider: "mock"
"#
    )
}

pub(crate) fn seed_hybrid_memory_corpus(workspace_root: &Path) {
    let memory_root = workspace_root.join(".openjarvis/memory");
    let documents = [
        (
            memory_root.join("passive/preferences/semantic-style.md"),
            r#"---
title: "回答风格约定"
created_at: 2026-04-01T10:00:00Z
updated_at: 2026-04-01T10:00:00Z
---
输出尽量简洁，默认中文，并把重点放在结论前面。
"#,
        ),
        (
            memory_root.join("passive/preferences/semantic-style-duplicate.md"),
            r#"---
title: "表达方式备忘"
created_at: 2026-04-02T10:00:00Z
updated_at: 2026-04-02T10:00:00Z
---
默认中文，回答保持简洁，把要点提前说明。
"#,
        ),
        (
            memory_root.join("passive/preferences/semantic-style-fresh.md"),
            r#"---
title: "最新回答风格"
created_at: 2026-04-10T10:00:00Z
updated_at: 2026-04-18T10:00:00Z
---
最近更新：默认使用中文，回答保持简洁，先给结论再展开细节。
"#,
        ),
        (
            memory_root.join("passive/preferences/noise.md"),
            r#"---
title: "周末随笔"
created_at: 2026-04-03T10:00:00Z
updated_at: 2026-04-03T10:00:00Z
---
这是一篇纯噪声文档，只记录天气、早餐和周末散步。
"#,
        ),
        (
            memory_root.join("active/workflow/notion.md"),
            r#"---
title: "Notion 上传工作流"
created_at: 2026-04-05T10:00:00Z
updated_at: 2026-04-05T10:00:00Z
keywords:
  - notion
  - 上传
---
上传到 notion 时走用户自定义模板，保留原始链接。
"#,
        ),
    ];

    for (path, content) in documents {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("memory corpus parent directory should exist");
        }
        fs::write(path, content).expect("memory corpus document should be written");
    }
}

pub(crate) struct ScopedCurrentDir {
    original: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ScopedCurrentDir {
    fn enter(path: &Path) -> Self {
        static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = CURRENT_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original = current_dir().expect("current dir should resolve");
        set_current_dir(path).expect("test current dir should switch to fixture");
        Self {
            original,
            _guard: guard,
        }
    }
}

impl Drop for ScopedCurrentDir {
    fn drop(&mut self) {
        let _ = set_current_dir(&self.original);
    }
}

#[derive(Clone, Default)]
struct HybridMockServerState {
    records: Arc<AsyncMutex<HybridMockRecords>>,
    fail_embeddings: bool,
    fail_rerank: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct HybridMockRecords {
    pub(crate) embedding_models: Vec<String>,
    pub(crate) rerank_models: Vec<String>,
    pub(crate) embedding_batch_sizes: Vec<usize>,
}

pub(crate) struct HybridMockServer {
    address: SocketAddr,
    api_key_path: PathBuf,
    records: Arc<AsyncMutex<HybridMockRecords>>,
    task: JoinHandle<()>,
}

impl HybridMockServer {
    pub(crate) async fn start(fixture: &MemoryWorkspaceFixture) -> Self {
        Self::start_with_options(fixture, false, false).await
    }

    pub(crate) async fn start_with_options(
        fixture: &MemoryWorkspaceFixture,
        fail_embeddings: bool,
        fail_rerank: bool,
    ) -> Self {
        let state = HybridMockServerState {
            records: Arc::new(AsyncMutex::new(HybridMockRecords::default())),
            fail_embeddings,
            fail_rerank,
        };
        let app = Router::new()
            .route("/v1/embeddings", post(hybrid_embeddings_handler))
            .route("/v1/rerank", post(hybrid_rerank_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("hybrid mock server should bind");
        let address = listener
            .local_addr()
            .expect("hybrid mock server address should resolve");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("hybrid mock server should run");
        });
        let api_key_path = fixture.root.join("config/siliconflow.key");
        if let Some(parent) = api_key_path.parent() {
            fs::create_dir_all(parent).expect("hybrid mock api key parent should exist");
        }
        fs::write(&api_key_path, "test-siliconflow-key")
            .expect("hybrid mock api key should be written");

        Self {
            address,
            api_key_path,
            records: state.records,
            task,
        }
    }

    pub(crate) fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }

    pub(crate) fn api_key_path(&self) -> &Path {
        &self.api_key_path
    }

    pub(crate) async fn records(&self) -> HybridMockRecords {
        self.records.lock().await.clone()
    }
}

impl Drop for HybridMockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingRequestPayload {
    model: String,
    input: Value,
}

#[derive(Debug, Deserialize)]
struct RerankRequestPayload {
    model: String,
    query: String,
    documents: Vec<String>,
    top_n: usize,
}

async fn hybrid_embeddings_handler(
    State(state): State<HybridMockServerState>,
    headers: HeaderMap,
    Json(payload): Json<EmbeddingRequestPayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_bearer_auth(&headers)?;
    if state.fail_embeddings {
        return Err(error_response(
            StatusCode::BAD_GATEWAY,
            "mock embeddings provider failure",
        ));
    }

    let inputs = match payload.input {
        Value::String(input) => vec![input],
        Value::Array(values) => values
            .into_iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "input must be string"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "input must be string or array",
            ));
        }
    };
    {
        let mut records = state.records.lock().await;
        records.embedding_models.push(payload.model.clone());
        records.embedding_batch_sizes.push(inputs.len());
    }

    let data = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            json!({
                "object": "embedding",
                "index": index,
                "embedding": semantic_embedding(input),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "data": data })))
}

async fn hybrid_rerank_handler(
    State(state): State<HybridMockServerState>,
    headers: HeaderMap,
    Json(payload): Json<RerankRequestPayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_bearer_auth(&headers)?;
    if state.fail_rerank {
        return Err(error_response(
            StatusCode::BAD_GATEWAY,
            "mock rerank provider failure",
        ));
    }

    if payload.top_n == 0 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "top_n must be positive",
        ));
    }

    {
        let mut records = state.records.lock().await;
        records.rerank_models.push(payload.model.clone());
    }

    let query_vector = semantic_embedding(&payload.query);
    let mut results = payload
        .documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let document_vector = semantic_embedding(document);
            let mut relevance_score = dot_product(&query_vector, &document_vector);
            if normalized_text(document).contains("最新回答风格") {
                relevance_score += 0.05;
            }
            if normalized_text(document).contains("workflow") {
                relevance_score += 0.01;
            }
            json!({
                "index": index,
                "relevance_score": relevance_score,
            })
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right["relevance_score"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&left["relevance_score"].as_f64().unwrap_or_default())
    });
    results.truncate(payload.top_n.min(results.len()));
    Ok(Json(json!({ "results": results })))
}

fn require_bearer_auth(headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    let authorized = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("Bearer "));
    if authorized {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::UNAUTHORIZED,
            "missing bearer auth",
        ))
    }
}

fn error_response(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": { "message": message } })))
}

fn semantic_embedding(text: &str) -> Vec<f32> {
    let normalized = normalized_text(text);
    let preference = contains_any(
        &normalized,
        &[
            "中文",
            "简洁",
            "回答风格",
            "表达方式",
            "结论",
            "偏好",
            "默认用中文",
        ],
    ) as u8 as f32;
    let workflow = contains_any(&normalized, &["notion", "上传", "模板", "工作流"]) as u8 as f32;
    let noise = contains_any(&normalized, &["天气", "早餐", "噪声", "周末"]) as u8 as f32;
    let freshness = contains_any(&normalized, &["最新", "最近更新"]) as u8 as f32;
    let duplicate = contains_any(&normalized, &["表达方式备忘", "回答风格约定"]) as u8 as f32;
    vec![preference, workflow, noise, freshness, duplicate]
}

fn normalized_text(text: &str) -> String {
    text.to_ascii_lowercase()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| text.contains(&needle.to_ascii_lowercase()))
}

fn dot_product(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| (*left as f64) * (*right as f64))
        .sum()
}

pub(crate) fn write_memory_document(path: &Path, document: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("memory document parent should exist");
    }
    fs::write(path, document).expect("memory document should be written");
}

pub(crate) fn parse_tool_json(content: &str) -> Value {
    serde_json::from_str(content).expect("tool result should be valid json")
}

pub(crate) fn unique_paths(items: &[Value]) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| item["path"].as_str().map(str::to_string))
        .collect()
}

pub(crate) fn build_config_from_yaml(yaml: &str) -> AppConfig {
    AppConfig::from_yaml_str(yaml).expect("test app config should parse")
}
