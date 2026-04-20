//! Local derived embedding cache for hybrid memory retrieval.

use super::siliconflow::{HybridSearchCredentials, SiliconFlowClient};
use crate::agent::memory::MemoryType;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::debug;
use uuid::Uuid;

const RETRIEVAL_CACHE_DIR: &str = ".retrieval";

pub(crate) struct EmbeddingCacheSource<'a> {
    pub(crate) memory_type: MemoryType,
    pub(crate) path: &'a str,
    pub(crate) title: &'a str,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) keywords: &'a [String],
    pub(crate) content: &'a str,
    pub(crate) dense_text: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentEmbeddingCache {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct EmbeddingCacheEntry {
    memory_type: MemoryType,
    path: String,
    model: String,
    fingerprint: String,
    embedding: Vec<f32>,
}

impl DocumentEmbeddingCache {
    pub(crate) fn new(memory_root: PathBuf) -> Self {
        Self {
            root: memory_root.join(RETRIEVAL_CACHE_DIR),
        }
    }

    pub(crate) async fn load_or_refresh_embeddings(
        &self,
        client: &SiliconFlowClient,
        credentials: &HybridSearchCredentials,
        model: &str,
        documents: &[EmbeddingCacheSource<'_>],
    ) -> Result<Vec<Vec<f32>>> {
        let mut resolved_embeddings = vec![None::<Vec<f32>>; documents.len()];
        let mut missing_positions = Vec::<usize>::new();
        let mut missing_inputs = Vec::<String>::new();

        for (position, document) in documents.iter().enumerate() {
            let fingerprint = embedding_fingerprint(document);
            let cache_path = cache_entry_path(&self.root, model, document, &fingerprint);
            match read_cache_entry(&cache_path, document, model, &fingerprint)? {
                Some(embedding) => {
                    resolved_embeddings[position] = Some(embedding);
                }
                None => {
                    missing_positions.push(position);
                    missing_inputs.push(document.dense_text.to_string());
                }
            }
        }

        if !missing_positions.is_empty() {
            debug!(
                model,
                cache_root = %self.root.display(),
                refresh_count = missing_positions.len(),
                "refreshing memory embedding cache entries"
            );
            let refreshed_embeddings = client
                .embed_texts(credentials, model, &missing_inputs)
                .await
                .context("failed to refresh missing memory embeddings")?;
            if refreshed_embeddings.len() != missing_positions.len() {
                bail!(
                    "embedding provider returned {} vectors for {} requested memory documents",
                    refreshed_embeddings.len(),
                    missing_positions.len()
                );
            }

            for (embedding_index, position) in missing_positions.into_iter().enumerate() {
                let document = &documents[position];
                let fingerprint = embedding_fingerprint(document);
                let cache_path = cache_entry_path(&self.root, model, document, &fingerprint);
                let embedding = refreshed_embeddings[embedding_index].clone();
                write_cache_entry(&cache_path, document, model, &fingerprint, &embedding)?;
                cleanup_stale_cache_entries(&cache_path, document)?;
                resolved_embeddings[position] = Some(embedding);
            }
        }

        resolved_embeddings
            .into_iter()
            .enumerate()
            .map(|(position, embedding)| {
                embedding.ok_or_else(|| {
                    anyhow!("memory embedding cache did not resolve position `{position}`")
                })
            })
            .collect()
    }
}

fn read_cache_entry(
    cache_path: &Path,
    document: &EmbeddingCacheSource<'_>,
    model: &str,
    fingerprint: &str,
) -> Result<Option<Vec<f32>>> {
    if !cache_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(cache_path)
        .with_context(|| format!("failed to read embedding cache {}", cache_path.display()))?;
    let entry = serde_json::from_str::<EmbeddingCacheEntry>(&raw)
        .with_context(|| format!("failed to parse embedding cache {}", cache_path.display()))?;
    if entry.memory_type != document.memory_type
        || entry.path != document.path
        || entry.model != model
        || entry.fingerprint != fingerprint
        || entry.embedding.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(entry.embedding))
}

fn write_cache_entry(
    cache_path: &Path,
    document: &EmbeddingCacheSource<'_>,
    model: &str,
    fingerprint: &str,
    embedding: &[f32],
) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create memory retrieval cache directory {}",
                parent.display()
            )
        })?;
    }
    let entry = EmbeddingCacheEntry {
        memory_type: document.memory_type,
        path: document.path.to_string(),
        model: model.to_string(),
        fingerprint: fingerprint.to_string(),
        embedding: embedding.to_vec(),
    };
    fs::write(
        cache_path,
        serde_json::to_vec_pretty(&entry).context("failed to serialize embedding cache entry")?,
    )
    .with_context(|| format!("failed to write embedding cache {}", cache_path.display()))?;
    debug!(
        model,
        memory_type = document.memory_type.as_dir_name(),
        path = document.path,
        cache_path = %cache_path.display(),
        "wrote memory embedding cache entry"
    );
    Ok(())
}

fn cleanup_stale_cache_entries(
    cache_path: &Path,
    document: &EmbeddingCacheSource<'_>,
) -> Result<()> {
    let Some(parent) = cache_path.parent() else {
        return Ok(());
    };
    if !parent.exists() {
        return Ok(());
    }
    let file_name = Path::new(document.path)
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("memory path `{}` has no valid file name", document.path))?;
    let entry_prefix = format!("{file_name}.");
    for entry in fs::read_dir(parent)
        .with_context(|| format!("failed to read cache directory {}", parent.display()))?
    {
        let entry = entry
            .with_context(|| format!("failed to inspect cache directory {}", parent.display()))?;
        let candidate_path = entry.path();
        if candidate_path == cache_path {
            continue;
        }
        let candidate_name = candidate_path.file_name().and_then(|value| value.to_str());
        if candidate_name.is_some_and(|candidate_name| candidate_name.starts_with(&entry_prefix))
            && candidate_name.is_some_and(|candidate_name| candidate_name.ends_with(".json"))
        {
            let _ = fs::remove_file(&candidate_path);
        }
    }
    Ok(())
}

fn cache_entry_path(
    root: &Path,
    model: &str,
    document: &EmbeddingCacheSource<'_>,
    fingerprint: &str,
) -> PathBuf {
    let model_dir = sanitize_path_component(model);
    let relative_path = Path::new(document.path);
    let mut cache_path = root
        .join(model_dir)
        .join(document.memory_type.as_dir_name())
        .join(relative_path);
    let file_name = relative_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("memory");
    cache_path.set_file_name(format!("{file_name}.{fingerprint}.json"));
    cache_path
}

fn embedding_fingerprint(document: &EmbeddingCacheSource<'_>) -> String {
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        document.memory_type.as_dir_name(),
        document.path,
        document.title,
        document.updated_at.to_rfc3339(),
        document.keywords.join(","),
        document.content,
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, payload.as_bytes()).to_string()
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}
