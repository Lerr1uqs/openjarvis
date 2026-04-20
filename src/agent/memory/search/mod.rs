//! Hybrid memory retrieval runtime for the `memory_search` tool.

mod cache;
mod siliconflow;

use self::cache::{DocumentEmbeddingCache, EmbeddingCacheSource};
use self::siliconflow::{HybridSearchCredentials, RerankResult, SiliconFlowClient};
use super::repository::{
    MemoryDocument, MemoryDocumentSummary, MemoryRepository, MemorySearchResponse, MemoryType,
};
use crate::config::{AgentMemorySearchConfig, MemorySearchMode};
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use jieba_rs::Jieba;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

/// Search runtime that keeps the public `memory_search` tool contract stable while choosing the
/// internal retrieval strategy from config.
pub(crate) struct MemorySearchService {
    config: AgentMemorySearchConfig,
    client: SiliconFlowClient,
    jieba: Jieba,
}

struct PreparedHybridDocument {
    document: MemoryDocument,
    sparse_tokens: Vec<String>,
    dense_text: String,
}

#[derive(Clone, Copy)]
struct RankedCandidate {
    document_index: usize,
    rerank_score: f64,
}

#[derive(Clone, Copy)]
struct MmrRankedCandidate {
    document_index: usize,
    rerank_score: f64,
    mmr_score: f64,
}

impl MemorySearchService {
    /// Create one memory search runtime bound to the loaded config snapshot.
    pub(crate) fn new(config: AgentMemorySearchConfig) -> Result<Self> {
        let client = SiliconFlowClient::new(config.hybrid_config().base_url())?;
        info!(
            mode = ?config.mode(),
            hybrid_base_url = %config.hybrid_config().base_url(),
            hybrid_embedding_model = %config.hybrid_config().embedding_model(),
            hybrid_rerank_model = %config.hybrid_config().rerank_model(),
            "initialized memory search runtime"
        );
        Ok(Self {
            config,
            client,
            jieba: Jieba::new(),
        })
    }

    /// Execute one lexical or hybrid memory search without changing the public tool response
    /// schema.
    pub(crate) async fn search(
        &self,
        repository: &MemoryRepository,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<MemorySearchResponse> {
        match self.config.mode() {
            MemorySearchMode::Lexical => repository.search(query, memory_type, limit),
            MemorySearchMode::Hybrid => {
                self.search_hybrid(repository, query, memory_type, limit)
                    .await
            }
        }
    }

    async fn search_hybrid(
        &self,
        repository: &MemoryRepository,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<MemorySearchResponse> {
        let query = query.trim();
        if query.is_empty() {
            bail!("memory search query must not be blank");
        }
        if limit == 0 {
            bail!("memory search limit must be greater than 0");
        }

        let documents = repository.load_search_documents(memory_type)?;
        if documents.is_empty() {
            debug!(
                query,
                memory_type = memory_type.map(MemoryType::as_dir_name),
                "hybrid memory search skipped because no documents are available"
            );
            return Ok(MemorySearchResponse {
                query: query.to_string(),
                total_matches: 0,
                items: Vec::new(),
            });
        }

        let hybrid_config = self.config.hybrid_config();
        debug!(
            query,
            memory_type = memory_type.map(MemoryType::as_dir_name),
            limit,
            document_count = documents.len(),
            bm25_top_n = hybrid_config.bm25_top_n(),
            dense_top_n = hybrid_config.dense_top_n(),
            rerank_top_n = hybrid_config.rerank_top_n(),
            "starting hybrid memory search request"
        );

        let prepared_documents = documents
            .into_iter()
            .map(|document| PreparedHybridDocument {
                sparse_tokens: tokenize_text(&self.jieba, &build_sparse_text(&document)),
                dense_text: build_dense_text(&document),
                document,
            })
            .collect::<Vec<_>>();

        let query_tokens = tokenize_query(&self.jieba, query);
        let bm25_candidates = rank_bm25_candidates(
            &prepared_documents,
            &query_tokens,
            hybrid_config.bm25_top_n(),
        );
        let credentials = HybridSearchCredentials::load(hybrid_config.api_key_path())
            .with_context(|| {
                format!(
                    "failed to load SiliconFlow credentials from {}",
                    hybrid_config.api_key_path().display()
                )
            })?;
        let query_embedding = self
            .client
            .embed_texts(
                &credentials,
                hybrid_config.embedding_model(),
                &[query.to_string()],
            )
            .await
            .context("hybrid memory search failed to fetch query embedding")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("embedding provider returned no query embedding"))?;

        let dense_sources = prepared_documents
            .iter()
            .map(|document| EmbeddingCacheSource {
                memory_type: document.document.memory_type,
                path: &document.document.path,
                title: &document.document.metadata.title,
                updated_at: document.document.metadata.updated_at,
                keywords: &document.document.metadata.keywords,
                content: &document.document.content,
                dense_text: &document.dense_text,
            })
            .collect::<Vec<_>>();
        let embedding_cache = DocumentEmbeddingCache::new(repository.memory_root());
        let document_embeddings = embedding_cache
            .load_or_refresh_embeddings(
                &self.client,
                &credentials,
                hybrid_config.embedding_model(),
                &dense_sources,
            )
            .await
            .context("hybrid memory search failed to load document embeddings")?;
        let dense_candidates = rank_dense_candidates(
            &query_embedding,
            &document_embeddings,
            hybrid_config.dense_top_n(),
        )?;
        let fused_candidates =
            fuse_ranked_candidates(&bm25_candidates, &dense_candidates, hybrid_config.rrf_k());
        let total_matches = fused_candidates.len();
        if fused_candidates.is_empty() {
            debug!(
                query,
                memory_type = memory_type.map(MemoryType::as_dir_name),
                "hybrid memory search returned no candidates after recall"
            );
            return Ok(MemorySearchResponse {
                query: query.to_string(),
                total_matches: 0,
                items: Vec::new(),
            });
        }

        let rerank_limit = hybrid_config.rerank_top_n().min(fused_candidates.len());
        let rerank_candidates = fused_candidates[..rerank_limit]
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let rerank_results = self
            .client
            .rerank_documents(
                &credentials,
                hybrid_config.rerank_model(),
                query,
                &rerank_candidates
                    .iter()
                    .map(|(document_index, _)| {
                        prepared_documents[*document_index].dense_text.clone()
                    })
                    .collect::<Vec<_>>(),
                rerank_limit,
            )
            .await
            .context("hybrid memory search failed to rerank fused candidates")?;
        let reranked_candidates = apply_rerank_results(&rerank_candidates, &rerank_results)?;
        let mmr_candidates = apply_mmr(
            &reranked_candidates,
            &document_embeddings,
            hybrid_config.mmr_lambda(),
        )?;
        let mut final_candidates = mmr_candidates
            .into_iter()
            .map(|candidate| {
                let document = &prepared_documents[candidate.document_index].document;
                let freshness_score = freshness_decay(
                    document.metadata.updated_at,
                    hybrid_config.freshness_half_life_days(),
                );
                (
                    candidate.document_index,
                    candidate.rerank_score,
                    candidate.mmr_score,
                    freshness_score,
                    candidate.mmr_score * freshness_score,
                )
            })
            .collect::<Vec<_>>();
        final_candidates.sort_by(|left, right| {
            right
                .4
                .total_cmp(&left.4)
                .then_with(|| right.3.total_cmp(&left.3))
                .then_with(|| {
                    prepared_documents[right.0]
                        .document
                        .metadata
                        .updated_at
                        .cmp(&prepared_documents[left.0].document.metadata.updated_at)
                })
                .then_with(|| {
                    prepared_documents[left.0]
                        .document
                        .path
                        .cmp(&prepared_documents[right.0].document.path)
                })
        });

        let items = final_candidates
            .into_iter()
            .take(limit)
            .map(|(document_index, _, _, _, _)| {
                summary_from_document(&prepared_documents[document_index].document)
            })
            .collect::<Vec<_>>();
        debug!(
            query,
            memory_type = memory_type.map(MemoryType::as_dir_name),
            limit,
            bm25_candidates = bm25_candidates.len(),
            dense_candidates = dense_candidates.len(),
            fused_candidates = total_matches,
            reranked_candidates = reranked_candidates.len(),
            returned_items = items.len(),
            "completed hybrid memory search request"
        );
        Ok(MemorySearchResponse {
            query: query.to_string(),
            total_matches,
            items,
        })
    }
}

fn build_sparse_text(document: &MemoryDocument) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        document.metadata.title,
        document.metadata.keywords.join(" "),
        document.path,
        document.content,
    )
}

fn build_dense_text(document: &MemoryDocument) -> String {
    if document.metadata.keywords.is_empty() {
        format!("{}\n{}", document.metadata.title, document.content)
    } else {
        format!(
            "{}\n{}\n{}",
            document.metadata.title,
            document.metadata.keywords.join(" "),
            document.content
        )
    }
}

fn tokenize_query(jieba: &Jieba, query: &str) -> Vec<String> {
    let tokens = tokenize_text(jieba, query);
    if tokens.is_empty() {
        vec![query.trim().to_ascii_lowercase()]
    } else {
        tokens
    }
}

fn tokenize_text(jieba: &Jieba, text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw_token in jieba.cut(text, false) {
        push_normalized_token(raw_token, &mut tokens);
    }
    tokens
}

fn push_normalized_token(raw: &str, tokens: &mut Vec<String>) {
    let mut current = String::new();
    for character in raw.chars() {
        if character.is_alphanumeric() || is_cjk(character) {
            if character.is_ascii() {
                current.push(character.to_ascii_lowercase());
            } else {
                current.push(character);
            }
        } else if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
}

fn is_cjk(character: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&character) || ('\u{3400}'..='\u{4DBF}').contains(&character)
}

fn rank_bm25_candidates(
    documents: &[PreparedHybridDocument],
    query_tokens: &[String],
    top_n: usize,
) -> Vec<(usize, f64)> {
    if documents.is_empty() {
        return Vec::new();
    }

    let average_length = documents
        .iter()
        .map(|document| document.sparse_tokens.len() as f64)
        .sum::<f64>()
        / documents.len() as f64;
    let mut document_frequency = HashMap::<String, usize>::new();
    for document in documents {
        let unique_tokens = document
            .sparse_tokens
            .iter()
            .cloned()
            .collect::<HashSet<String>>();
        for token in unique_tokens {
            *document_frequency.entry(token).or_default() += 1;
        }
    }

    let mut scored = documents
        .iter()
        .enumerate()
        .filter_map(|(index, document)| {
            let score = bm25_score_for_document(
                query_tokens,
                &document.sparse_tokens,
                average_length,
                documents.len(),
                &document_frequency,
            );
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored.truncate(top_n);
    scored
}

fn bm25_score_for_document(
    query_tokens: &[String],
    document_tokens: &[String],
    average_length: f64,
    document_count: usize,
    document_frequency: &HashMap<String, usize>,
) -> f64 {
    const K1: f64 = 1.5;
    const B: f64 = 0.75;

    if document_tokens.is_empty() {
        return 0.0;
    }

    let mut term_frequency = HashMap::<&str, usize>::new();
    for token in document_tokens {
        *term_frequency.entry(token.as_str()).or_default() += 1;
    }

    let document_length = document_tokens.len() as f64;
    query_tokens.iter().fold(0.0, |accumulator, token| {
        let Some(term_count) = term_frequency.get(token.as_str()) else {
            return accumulator;
        };
        let document_frequency = *document_frequency.get(token).unwrap_or(&0) as f64;
        if document_frequency == 0.0 {
            return accumulator;
        }
        let idf = (1.0
            + ((document_count as f64 - document_frequency + 0.5) / (document_frequency + 0.5)))
            .ln();
        let tf = *term_count as f64;
        let denominator = tf + K1 * (1.0 - B + B * (document_length / average_length.max(1.0)));
        accumulator + idf * ((tf * (K1 + 1.0)) / denominator)
    })
}

fn rank_dense_candidates(
    query_embedding: &[f32],
    document_embeddings: &[Vec<f32>],
    top_n: usize,
) -> Result<Vec<(usize, f64)>> {
    let mut scored = document_embeddings
        .iter()
        .enumerate()
        .map(|(index, embedding)| Ok((index, cosine_similarity(query_embedding, embedding)?)))
        .collect::<Result<Vec<_>>>()?;
    scored.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored.truncate(top_n);
    Ok(scored)
}

fn fuse_ranked_candidates(
    bm25_candidates: &[(usize, f64)],
    dense_candidates: &[(usize, f64)],
    rrf_k: usize,
) -> Vec<(usize, f64)> {
    let mut fused_scores = HashMap::<usize, f64>::new();
    for (rank, (document_index, _)) in bm25_candidates.iter().enumerate() {
        *fused_scores.entry(*document_index).or_default() += 1.0 / (rrf_k + rank + 1) as f64;
    }
    for (rank, (document_index, _)) in dense_candidates.iter().enumerate() {
        *fused_scores.entry(*document_index).or_default() += 1.0 / (rrf_k + rank + 1) as f64;
    }

    let mut fused = fused_scores.into_iter().collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    fused
}

fn apply_rerank_results(
    rerank_candidates: &[(usize, f64)],
    rerank_results: &[RerankResult],
) -> Result<Vec<RankedCandidate>> {
    if rerank_candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut by_index = HashMap::<usize, f64>::new();
    for result in rerank_results {
        if result.index >= rerank_candidates.len() {
            bail!(
                "rerank provider returned out-of-bounds candidate index `{}` for {} candidates",
                result.index,
                rerank_candidates.len()
            );
        }
        by_index.insert(result.index, result.relevance_score);
    }
    if by_index.len() != rerank_candidates.len() {
        bail!(
            "rerank provider returned {} results for {} candidates",
            by_index.len(),
            rerank_candidates.len()
        );
    }

    let mut ranked = rerank_candidates
        .iter()
        .enumerate()
        .map(|(result_index, (document_index, _))| {
            Ok(RankedCandidate {
                document_index: *document_index,
                rerank_score: *by_index.get(&result_index).ok_or_else(|| {
                    anyhow!("rerank provider omitted candidate index `{result_index}`")
                })?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ranked.sort_by(|left, right| {
        right
            .rerank_score
            .total_cmp(&left.rerank_score)
            .then_with(|| left.document_index.cmp(&right.document_index))
    });
    Ok(ranked)
}

fn apply_mmr(
    reranked_candidates: &[RankedCandidate],
    document_embeddings: &[Vec<f32>],
    lambda: f64,
) -> Result<Vec<MmrRankedCandidate>> {
    let mut remaining = reranked_candidates.to_vec();
    let mut selected = Vec::<MmrRankedCandidate>::with_capacity(reranked_candidates.len());
    while !remaining.is_empty() {
        let next_position = if selected.is_empty() {
            0
        } else {
            remaining
                .iter()
                .enumerate()
                .map(|(position, candidate)| {
                    let max_similarity =
                        selected.iter().try_fold(0.0_f64, |accumulator, selected| {
                            let similarity = cosine_similarity(
                                &document_embeddings[candidate.document_index],
                                &document_embeddings[selected.document_index],
                            )?;
                            Ok::<f64, anyhow::Error>(accumulator.max(similarity))
                        })?;
                    let mmr_score =
                        lambda * candidate.rerank_score - (1.0 - lambda) * max_similarity;
                    Ok::<(usize, f64), anyhow::Error>((position, mmr_score))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .max_by(|left, right| {
                    left.1
                        .total_cmp(&right.1)
                        .then_with(|| right.0.cmp(&left.0))
                })
                .map(|(position, _)| position)
                .unwrap_or(0)
        };

        let candidate = remaining.remove(next_position);
        let max_similarity = selected.iter().try_fold(0.0_f64, |accumulator, selected| {
            let similarity = cosine_similarity(
                &document_embeddings[candidate.document_index],
                &document_embeddings[selected.document_index],
            )?;
            Ok::<f64, anyhow::Error>(accumulator.max(similarity))
        })?;
        let mmr_score = if selected.is_empty() {
            candidate.rerank_score
        } else {
            lambda * candidate.rerank_score - (1.0 - lambda) * max_similarity
        };
        selected.push(MmrRankedCandidate {
            document_index: candidate.document_index,
            rerank_score: candidate.rerank_score,
            mmr_score,
        });
    }
    Ok(selected)
}

fn freshness_decay(updated_at: chrono::DateTime<Utc>, half_life_days: u64) -> f64 {
    let age_seconds = (Utc::now() - updated_at).num_seconds().max(0) as f64;
    let age_days = age_seconds / 86_400.0;
    0.5_f64.powf(age_days / half_life_days as f64)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f64> {
    if left.len() != right.len() {
        bail!(
            "embedding dimension mismatch: left={}, right={}",
            left.len(),
            right.len()
        );
    }
    if left.is_empty() {
        bail!("embedding vector must not be empty");
    }

    let (mut dot, mut left_norm, mut right_norm) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        let left_value = *left_value as f64;
        let right_value = *right_value as f64;
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        bail!("embedding vector norm must not be zero");
    }
    Ok(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

fn summary_from_document(document: &MemoryDocument) -> MemoryDocumentSummary {
    MemoryDocumentSummary {
        memory_type: document.memory_type,
        path: document.path.clone(),
        title: document.metadata.title.clone(),
        updated_at: document.metadata.updated_at,
        keywords: document.metadata.keywords.clone(),
    }
}
