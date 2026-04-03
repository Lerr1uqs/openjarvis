//! Experimental standalone auto-compactor for persisted thread history.
//!
//! `AutoCompactorEx` keeps the responsibility narrow:
//! 1. load one thread snapshot from `SessionManager` or accept one detached `ThreadContext`;
//! 2. compact only the persisted `chat` history through the shared compact stack;
//! 3. materialize the compacted history back into one updated `ThreadContext`.
//!
//! It does not decide when auto-compact should run. That policy still belongs to runtime callers.

use crate::{
    compact::{
        CompactAllChatStrategy, CompactManager, CompactProvider, CompactStrategy,
        CompactionOutcome, LLMCompactProvider,
    },
    config::AgentCompactConfig,
    llm::LLMProvider,
    session::{SessionManager, ThreadLocator},
    thread::ThreadContext,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{info, warn};

/// Result of one `AutoCompactorEx` run.
#[derive(Debug, Clone)]
pub struct AutoCompactorExOutcome {
    pub compacted_at: DateTime<Utc>,
    pub thread_context: ThreadContext,
    pub compaction: CompactionOutcome,
}

/// Standalone thread-history compactor that can work on detached snapshots or persisted threads.
pub struct AutoCompactorEx {
    compact_config: AgentCompactConfig,
    session_manager: Arc<SessionManager>,
    compact_manager: CompactManager,
}

impl AutoCompactorEx {
    /// Create one auto-compactor with the default `compact_all_chat` strategy.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::feature::auto_compactor_v1::AutoCompactorEx,
    ///     compact::{CompactSummary, StaticCompactProvider},
    ///     config::AgentCompactConfig,
    ///     session::SessionManager,
    /// };
    /// use serde_json::json;
    /// use std::sync::Arc;
    ///
    /// let compact_config: AgentCompactConfig =
    ///     serde_json::from_value(json!({ "enabled": true })).expect("config should parse");
    /// let compactor = AutoCompactorEx::new(
    ///     Arc::new(SessionManager::new()),
    ///     compact_config,
    ///     Arc::new(StaticCompactProvider::new(CompactSummary {
    ///         compacted_assistant: "这是压缩后的上下文：目标未变。".to_string(),
    ///     })),
    /// );
    ///
    /// assert!(compactor.compact_enabled());
    /// ```
    pub fn new(
        session_manager: Arc<SessionManager>,
        compact_config: AgentCompactConfig,
        compact_provider: Arc<dyn CompactProvider>,
    ) -> Self {
        Self::with_strategy(
            session_manager,
            compact_config,
            compact_provider,
            Arc::new(CompactAllChatStrategy),
        )
    }

    /// Create one auto-compactor with an explicit compact strategy.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::feature::auto_compactor_v1::AutoCompactorEx,
    ///     compact::{CompactAllChatStrategy, CompactSummary, StaticCompactProvider},
    ///     config::AgentCompactConfig,
    ///     session::SessionManager,
    /// };
    /// use serde_json::json;
    /// use std::sync::Arc;
    ///
    /// let compact_config: AgentCompactConfig =
    ///     serde_json::from_value(json!({ "enabled": true })).expect("config should parse");
    /// let _compactor = AutoCompactorEx::with_strategy(
    ///     Arc::new(SessionManager::new()),
    ///     compact_config,
    ///     Arc::new(StaticCompactProvider::new(CompactSummary {
    ///         compacted_assistant: "这是压缩后的上下文：目标未变。".to_string(),
    ///     })),
    ///     Arc::new(CompactAllChatStrategy),
    /// );
    /// ```
    pub fn with_strategy(
        session_manager: Arc<SessionManager>,
        compact_config: AgentCompactConfig,
        compact_provider: Arc<dyn CompactProvider>,
        strategy: Arc<dyn CompactStrategy>,
    ) -> Self {
        Self {
            compact_config,
            session_manager,
            compact_manager: CompactManager::new(compact_provider, strategy),
        }
    }

    /// Create one auto-compactor backed by the shared model provider.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::feature::auto_compactor_v1::AutoCompactorEx,
    ///     config::AgentCompactConfig,
    ///     llm::MockLLMProvider,
    ///     session::SessionManager,
    /// };
    /// use serde_json::json;
    /// use std::sync::Arc;
    ///
    /// let compact_config: AgentCompactConfig =
    ///     serde_json::from_value(json!({ "enabled": true })).expect("config should parse");
    /// let _compactor = AutoCompactorEx::from_model_provider(
    ///     Arc::new(SessionManager::new()),
    ///     compact_config,
    ///     Arc::new(MockLLMProvider::new("{\"compacted_assistant\":\"这是压缩后的上下文：继续处理当前任务。\"}")),
    /// );
    /// ```
    pub fn from_model_provider(
        session_manager: Arc<SessionManager>,
        compact_config: AgentCompactConfig,
        provider: Arc<dyn LLMProvider>,
    ) -> Self {
        Self::new(
            session_manager,
            compact_config,
            Arc::new(LLMCompactProvider::new(provider)),
        )
    }

    /// Return whether this compactor is allowed to execute compact runs.
    pub fn compact_enabled(&self) -> bool {
        self.compact_config.enabled()
    }

    /// Compact one detached thread snapshot and return the updated snapshot.
    ///
    /// `thread_context` should be one persisted snapshot or one completed-turn snapshot.
    /// Pending live chat messages are intentionally rejected because compact only replaces stored
    /// history; dropping still-running turn messages here would be unsafe.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::feature::auto_compactor_v1::AutoCompactorEx,
    ///     compact::{CompactSummary, StaticCompactProvider},
    ///     config::AgentCompactConfig,
    ///     context::{ChatMessage, ChatMessageRole},
    ///     session::SessionManager,
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    /// use serde_json::json;
    /// use std::sync::Arc;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let compact_config: AgentCompactConfig =
    ///     serde_json::from_value(json!({ "enabled": true })).expect("config should parse");
    /// let compactor = AutoCompactorEx::new(
    ///     Arc::new(SessionManager::new()),
    ///     compact_config,
    ///     Arc::new(StaticCompactProvider::new(CompactSummary {
    ///         compacted_assistant: "这是压缩后的上下文：用户要我继续实现 auto compactor。".to_string(),
    ///     })),
    /// );
    /// let now = Utc::now();
    /// let mut thread_context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread_context.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// let outcome = compactor.compact_thread_context(&thread_context).await?;
    /// assert!(outcome.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn compact_thread_context(
        &self,
        thread_context: &ThreadContext,
    ) -> Result<Option<AutoCompactorExOutcome>> {
        if !self.compact_enabled() {
            info!(
                thread_id = %thread_context.locator.thread_id,
                "skipping auto compactor v1 because compact is disabled"
            );
            return Ok(None);
        }

        if !thread_context.pending_chat_messages().is_empty() {
            warn!(
                thread_id = %thread_context.locator.thread_id,
                pending_chat_message_count = thread_context.pending_chat_messages().len(),
                "skipping auto compactor v1 because thread context still has pending live chat messages"
            );
            return Ok(None);
        }

        let compacted_at = Utc::now();
        let Some(compaction) = self
            .compact_manager
            .compact_thread(&thread_context.to_conversation_thread(), compacted_at)
            .await
            .with_context(|| {
                format!(
                    "failed to compact detached thread `{}`",
                    thread_context.locator.thread_id
                )
            })?
        else {
            info!(
                thread_id = %thread_context.locator.thread_id,
                "auto compactor v1 found no persisted chat history to compact"
            );
            return Ok(None);
        };

        let outcome = materialize_compacted_outcome(thread_context, compacted_at, compaction);
        info!(
            thread_id = %outcome.thread_context.locator.thread_id,
            source_turn_count = outcome.compaction.plan.source_turn_ids.len(),
            after_turn_count = outcome.thread_context.conversation.turns.len(),
            "auto compactor v1 compacted detached thread context"
        );
        Ok(Some(outcome))
    }

    /// Load one persisted thread, compact it, and write the compacted snapshot back.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::feature::auto_compactor_v1::AutoCompactorEx,
    ///     compact::{CompactSummary, StaticCompactProvider},
    ///     config::AgentCompactConfig,
    ///     context::{ChatMessage, ChatMessageRole},
    ///     model::{IncomingMessage, ReplyTarget},
    ///     session::{SessionManager, ThreadLocator},
    /// };
    /// use serde_json::json;
    /// use std::sync::Arc;
    /// use uuid::Uuid;
    ///
    /// # async fn demo() -> anyhow::Result<()> {
    /// let manager = Arc::new(SessionManager::new());
    /// let compact_config: AgentCompactConfig =
    ///     serde_json::from_value(json!({ "enabled": true })).expect("config should parse");
    /// let compactor = AutoCompactorEx::new(
    ///     Arc::clone(&manager),
    ///     compact_config,
    ///     Arc::new(StaticCompactProvider::new(CompactSummary {
    ///         compacted_assistant: "这是压缩后的上下文：继续处理当前任务。".to_string(),
    ///     })),
    /// );
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_1".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: Some("thread_ext".to_string()),
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    /// let locator = manager.load_or_create_thread(&incoming).await?;
    /// manager
    ///     .store_turn(
    ///         &locator,
    ///         Some("msg_1".to_string()),
    ///         vec![ChatMessage::new(ChatMessageRole::User, "hello", incoming.received_at)],
    ///         incoming.received_at,
    ///         incoming.received_at,
    ///     )
    ///     .await?;
    ///
    /// let outcome = compactor.compact_persisted_thread(&locator).await?;
    /// assert!(outcome.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn compact_persisted_thread(
        &self,
        locator: &ThreadLocator,
    ) -> Result<Option<AutoCompactorExOutcome>> {
        let Some(thread_context) = self
            .session_manager
            .load_thread_context(locator)
            .await
            .with_context(|| {
                format!(
                    "failed to load persisted thread `{}` before compact",
                    locator.thread_id
                )
            })?
        else {
            warn!(
                thread_id = %locator.thread_id,
                external_thread_id = %locator.external_thread_id,
                "skipping auto compactor v1 because persisted thread snapshot was not found"
            );
            return Ok(None);
        };

        let Some(outcome) = self.compact_thread_context(&thread_context).await? else {
            return Ok(None);
        };

        // `SessionManager` 当前只有一个“整快照写回”入口。这里依赖调用方对同一线程串行执行；
        // 如果后续出现独立的 history overwrite API，应切换过去，避免冲突恢复只保留 state。
        self.session_manager
            .store_thread_context(
                locator,
                outcome.thread_context.clone(),
                outcome.compacted_at,
            )
            .await
            .with_context(|| {
                format!(
                    "failed to persist compacted thread `{}` back to session manager",
                    locator.thread_id
                )
            })?;

        info!(
            thread_id = %locator.thread_id,
            source_turn_count = outcome.compaction.plan.source_turn_ids.len(),
            after_turn_count = outcome.thread_context.conversation.turns.len(),
            "auto compactor v1 persisted compacted thread snapshot"
        );
        Ok(Some(outcome))
    }
}

fn materialize_compacted_outcome(
    thread_context: &ThreadContext,
    compacted_at: DateTime<Utc>,
    compaction: CompactionOutcome,
) -> AutoCompactorExOutcome {
    let feature_slots = thread_context.features_system_prompt().clone();
    let memory_messages = thread_context.request_memory_messages().to_vec();
    let mut compacted_thread_context = thread_context.clone();

    compacted_thread_context
        .overwrite_active_history_from_conversation_thread(&compaction.compacted_thread);

    // 覆盖 active history 会清空请求期的可见视图，这里把仍然有效的 transient prompt 恢复回来。
    compacted_thread_context.rebuild_features_system_prompt(feature_slots);
    compacted_thread_context.replace_live_system_messages(Vec::new());
    compacted_thread_context.rebuild_live_memory_messages(memory_messages);

    AutoCompactorExOutcome {
        compacted_at,
        thread_context: compacted_thread_context,
        compaction,
    }
}

#[cfg(test)]
mod tests {
    use super::AutoCompactorEx;
    use crate::{
        compact::{CompactSummary, StaticCompactProvider},
        config::AgentCompactConfig,
        context::{ChatMessage, ChatMessageRole},
        session::SessionManager,
        thread::{ThreadContext, ThreadContextLocator, ThreadFeaturesSystemPrompt},
    };
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;

    fn enabled_compact_config() -> AgentCompactConfig {
        serde_json::from_value(json!({ "enabled": true })).expect("config should parse")
    }

    fn build_compactor(summary: &str) -> AutoCompactorEx {
        AutoCompactorEx::new(
            Arc::new(SessionManager::new()),
            enabled_compact_config(),
            Arc::new(StaticCompactProvider::new(CompactSummary {
                compacted_assistant: summary.to_string(),
            })),
        )
    }

    fn build_thread_context(now: chrono::DateTime<Utc>) -> ThreadContext {
        ThreadContext::new(
            ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
            now,
        )
    }

    #[tokio::test]
    async fn compact_thread_context_preserves_request_time_views() {
        // 测试：compact 只替换持久化 chat 历史，但保留系统快照、feature prompt 和 memory 视图。
        let now = Utc::now();
        let compactor = build_compactor("这是压缩后的上下文：任务仍然是继续实现。");
        let mut thread_context = build_thread_context(now);
        let _ = thread_context.ensure_system_prompt_snapshot("system prompt", now);
        thread_context.replace_request_memory_messages(vec![ChatMessage::new(
            ChatMessageRole::Memory,
            "remember this",
            now,
        )]);
        let mut feature_slots = ThreadFeaturesSystemPrompt::default();
        feature_slots.auto_compact.push(ChatMessage::new(
            ChatMessageRole::System,
            "auto compact hint",
            now,
        ));
        thread_context.rebuild_features_system_prompt(feature_slots);
        thread_context.store_turn(
            Some("msg_1".to_string()),
            vec![
                ChatMessage::new(ChatMessageRole::User, "first", now),
                ChatMessage::new(ChatMessageRole::Assistant, "first-reply", now),
            ],
            now,
            now,
        );
        thread_context.store_turn(
            Some("msg_2".to_string()),
            vec![ChatMessage::new(ChatMessageRole::User, "second", now)],
            now,
            now,
        );

        let outcome = compactor
            .compact_thread_context(&thread_context)
            .await
            .expect("compact should succeed")
            .expect("compact should produce a result");

        assert_eq!(outcome.thread_context.conversation.turns.len(), 1);
        assert_eq!(outcome.thread_context.load_messages().len(), 2);
        assert_eq!(
            outcome
                .thread_context
                .request_context_system_messages()
                .len(),
            1
        );
        assert_eq!(outcome.thread_context.request_memory_messages().len(), 1);
        assert_eq!(
            outcome
                .thread_context
                .features_system_prompt()
                .auto_compact
                .len(),
            1
        );
        assert!(
            outcome
                .thread_context
                .messages()
                .iter()
                .any(|message| message.content.contains("这是压缩后的上下文"))
        );
    }

    #[tokio::test]
    async fn compact_thread_context_skips_pending_live_chat_messages() {
        // 测试：如果当前线程还有未落库的 live chat，compactor 必须拒绝执行，避免误删进行中的 turn。
        let now = Utc::now();
        let compactor = build_compactor("这是压缩后的上下文：继续。");
        let mut thread_context = build_thread_context(now);
        thread_context.store_turn(
            Some("msg_1".to_string()),
            vec![ChatMessage::new(ChatMessageRole::User, "persisted", now)],
            now,
            now,
        );
        thread_context.push_message(ChatMessage::new(
            ChatMessageRole::User,
            "still running",
            now,
        ));

        let outcome = compactor
            .compact_thread_context(&thread_context)
            .await
            .expect("compact should not error");

        assert!(outcome.is_none());
    }
}
