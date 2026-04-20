//! SiliconFlow-compatible embedding and rerank client used by hybrid memory retrieval.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use tracing::debug;

#[derive(Debug, Clone)]
pub(crate) struct HybridSearchCredentials {
    api_key: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SiliconFlowClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RerankResult {
    pub(crate) index: usize,
    pub(crate) relevance_score: f64,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
    return_documents: bool,
}

#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResponseItem>,
}

#[derive(Debug, Deserialize)]
struct RerankResponseItem {
    index: usize,
    relevance_score: f64,
}

impl HybridSearchCredentials {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let expanded_path = expand_home_dir(path)?;
        let api_key = fs::read_to_string(&expanded_path)
            .with_context(|| {
                format!(
                    "failed to read SiliconFlow api key from {}",
                    expanded_path.display()
                )
            })?
            .trim()
            .to_string();
        if api_key.is_empty() {
            bail!(
                "SiliconFlow api key file {} must not be blank",
                expanded_path.display()
            );
        }
        debug!(
            api_key_path = %expanded_path.display(),
            "loaded SiliconFlow hybrid retrieval credentials"
        );
        Ok(Self { api_key })
    }
}

impl SiliconFlowClient {
    pub(crate) fn new(base_url: &str) -> Result<Self> {
        if base_url.trim().is_empty() {
            bail!("SiliconFlow base_url must not be blank");
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .build()
                .context("failed to build SiliconFlow http client")?,
        })
    }

    pub(crate) async fn embed_texts(
        &self,
        credentials: &HybridSearchCredentials,
        model: &str,
        input: &[String],
    ) -> Result<Vec<Vec<f32>>> {
        if model.trim().is_empty() {
            bail!("hybrid embedding model must not be blank");
        }
        if input.is_empty() {
            bail!("embedding request input must not be empty");
        }

        let endpoint = format!("{}/embeddings", self.base_url);
        debug!(
            endpoint = %endpoint,
            model,
            batch_size = input.len(),
            "sending SiliconFlow embedding request"
        );
        let response = self
            .client
            .post(&endpoint)
            .headers(authorization_headers(credentials)?)
            .json(&EmbeddingRequest { model, input })
            .send()
            .await
            .with_context(|| {
                format!("failed to call SiliconFlow embeddings endpoint {endpoint}")
            })?;
        let status = response.status();
        let body = response.text().await.with_context(|| {
            format!("failed to read SiliconFlow embeddings response from {endpoint}")
        })?;
        if !status.is_success() {
            bail!("SiliconFlow embeddings request failed with status {status}: {body}");
        }

        let mut payload = serde_json::from_str::<EmbeddingResponse>(&body)
            .context("SiliconFlow embeddings response payload is invalid")?;
        payload
            .data
            .sort_by(|left, right| left.index.cmp(&right.index));
        if payload.data.len() != input.len() {
            bail!(
                "SiliconFlow embeddings response returned {} items for {} inputs",
                payload.data.len(),
                input.len()
            );
        }
        let embeddings = payload
            .data
            .into_iter()
            .map(|item| {
                if item.embedding.is_empty() {
                    Err(anyhow!(
                        "SiliconFlow embeddings response contains one empty embedding vector"
                    ))
                } else {
                    Ok(item.embedding)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        debug!(
            endpoint = %endpoint,
            model,
            batch_size = embeddings.len(),
            "completed SiliconFlow embedding request"
        );
        Ok(embeddings)
    }

    pub(crate) async fn rerank_documents(
        &self,
        credentials: &HybridSearchCredentials,
        model: &str,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>> {
        if model.trim().is_empty() {
            bail!("hybrid rerank model must not be blank");
        }
        if query.trim().is_empty() {
            bail!("hybrid rerank query must not be blank");
        }
        if documents.is_empty() {
            bail!("hybrid rerank documents must not be empty");
        }
        if top_n == 0 {
            bail!("hybrid rerank top_n must be greater than 0");
        }

        let endpoint = format!("{}/rerank", self.base_url);
        debug!(
            endpoint = %endpoint,
            model,
            document_count = documents.len(),
            top_n,
            "sending SiliconFlow rerank request"
        );
        let response = self
            .client
            .post(&endpoint)
            .headers(authorization_headers(credentials)?)
            .json(&RerankRequest {
                model,
                query,
                documents,
                top_n,
                return_documents: false,
            })
            .send()
            .await
            .with_context(|| format!("failed to call SiliconFlow rerank endpoint {endpoint}"))?;
        let status = response.status();
        let body = response.text().await.with_context(|| {
            format!("failed to read SiliconFlow rerank response from {endpoint}")
        })?;
        if !status.is_success() {
            bail!("SiliconFlow rerank request failed with status {status}: {body}");
        }

        let payload = serde_json::from_str::<RerankResponse>(&body)
            .context("SiliconFlow rerank response payload is invalid")?;
        if payload.results.is_empty() {
            bail!("SiliconFlow rerank response returned no results");
        }
        let mut results = payload
            .results
            .into_iter()
            .map(|item| RerankResult {
                index: item.index,
                relevance_score: item.relevance_score,
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .relevance_score
                .total_cmp(&left.relevance_score)
                .then_with(|| left.index.cmp(&right.index))
        });
        debug!(
            endpoint = %endpoint,
            model,
            result_count = results.len(),
            "completed SiliconFlow rerank request"
        );
        Ok(results)
    }
}

fn authorization_headers(credentials: &HybridSearchCredentials) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", credentials.api_key))
            .context("failed to encode SiliconFlow authorization header")?,
    );
    Ok(headers)
}

fn expand_home_dir(path: &Path) -> Result<PathBuf> {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return resolve_home_dir();
    }
    if let Some(suffix) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return Ok(resolve_home_dir()?.join(suffix));
    }
    Ok(path.to_path_buf())
}

fn resolve_home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME environment variable is not set"))
}
