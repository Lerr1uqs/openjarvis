//! Thread-first aggregate model and atomic thread-owned persistence helpers.

use crate::agent::{
    FeaturePromptRebuilder, MemoryRepository, ToolCallRequest, ToolCallResult, ToolDefinition,
    ToolRegistry,
};
use crate::context::{ChatMessage, ChatMessageRole};
use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

const OPENJARVIS_THREAD_ID_NAMESPACE: Uuid =
    Uuid::from_u128(0x7f4b2e8d_5d33_4f51_9c27_9c5d7d76c1a1);

/// Derive the stable internal thread id from one normalized thread key.
///
/// `thread_key` follows the contract `user:channel:external_thread_id`.
///
/// # 示例
/// ```rust
/// use openjarvis::thread::derive_internal_thread_id;
///
/// let thread_id = derive_internal_thread_id("ou_xxx:feishu:thread_ext");
/// assert_eq!(
///     thread_id,
///     derive_internal_thread_id("ou_xxx:feishu:thread_ext")
/// );
/// ```
pub fn derive_internal_thread_id(thread_key: &str) -> Uuid {
    Uuid::new_v5(&OPENJARVIS_THREAD_ID_NAMESPACE, thread_key.as_bytes())
}

fn default_tool_event_metadata() -> Value {
    json!({})
}

fn normalize_loaded_toolsets<I>(loaded_toolsets: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut names = loaded_toolsets
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThreadToolEventKind {
    LoadToolset,
    UnloadToolset,
    ExecuteTool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadToolEvent {
    pub id: Uuid,
    pub kind: ThreadToolEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolset_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
    #[serde(default)]
    pub is_error: bool,
    pub recorded_at: DateTime<Utc>,
}

impl ThreadToolEvent {
    /// Create one structured thread tool event.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::thread::{ThreadToolEvent, ThreadToolEventKind};
    ///
    /// let event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, Utc::now());
    /// assert_eq!(event.kind, ThreadToolEventKind::ExecuteTool);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new(kind: ThreadToolEventKind, recorded_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            toolset_name: None,
            tool_name: None,
            tool_call_id: None,
            arguments: None,
            metadata: default_tool_event_metadata(),
            is_error: false,
            recorded_at,
        }
    }
}

/// Stable runtime locator shared by session, command, and agent execution on one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadContextLocator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub channel: String,
    pub user_id: String,
    pub external_thread_id: String,
    pub thread_id: String,
}

impl ThreadContextLocator {
    /// Build one explicit thread-context locator.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::thread::ThreadContextLocator;
    ///
    /// let locator = ThreadContextLocator::new(
    ///     Some("session-1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    /// );
    /// assert_eq!(locator.external_thread_id, "thread_ext");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new(
        session_id: Option<String>,
        channel: impl Into<String>,
        user_id: impl Into<String>,
        external_thread_id: impl Into<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            channel: channel.into(),
            user_id: user_id.into(),
            external_thread_id: external_thread_id.into(),
            thread_id: thread_id.into(),
        }
    }

    /// Return the normalized thread key used to derive the internal thread id.
    ///
    /// `thread_key` follows the contract `user:channel:external_thread_id`.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::thread::ThreadContextLocator;
    ///
    /// let locator = ThreadContextLocator::new(
    ///     Some("session-1".to_string()),
    ///     "feishu",
    ///     "ou_xxx",
    ///     "thread_ext",
    ///     "00000000-0000-0000-0000-000000000001",
    /// );
    /// assert_eq!(locator.thread_key(), "ou_xxx:feishu:thread_ext");
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn thread_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.user_id, self.channel, self.external_thread_id
        )
    }
}

/// Thread lifecycle state that belongs to persisted thread state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadLifecycleState {
    #[serde(default)]
    pub initialized: bool,
}

/// Thread feature flags and runtime feature overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadFeatureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_override: Option<bool>,
}

/// Thread-scoped tool runtime state owned by `Thread`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadToolState {
    #[serde(default)]
    pub loaded_toolsets: Vec<String>,
    #[serde(default)]
    pub tool_events: Vec<ThreadToolEvent>,
}

/// One pending approval request reserved for future thread-scoped policy flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalRequest {
    pub id: Uuid,
    pub action: String,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
    pub requested_at: DateTime<Utc>,
}

/// One persisted approval decision reserved for future thread-scoped policy flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalDecision {
    pub request_id: Uuid,
    pub approved: bool,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
    pub decided_at: DateTime<Utc>,
}

/// Thread-scoped approval state reserved for policy and approval workflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadApprovalState {
    #[serde(default)]
    pub pending: Vec<ThreadApprovalRequest>,
    #[serde(default)]
    pub decisions: Vec<ThreadApprovalDecision>,
}

/// Full thread-scoped non-message state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadState {
    #[serde(default)]
    pub lifecycle: ThreadLifecycleState,
    #[serde(default)]
    pub features: ThreadFeatureState,
    #[serde(default)]
    pub tools: ThreadToolState,
    #[serde(default)]
    pub approval: ThreadApprovalState,
}

/// Persisted message domain owned by one thread aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadContext {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ThreadContext {
    /// Create one empty persisted thread context.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Export all persisted messages currently owned by the thread.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }
}

/// Persisted thread snapshot written by thread-owned CAS mutations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedThreadSnapshot {
    pub thread: ThreadContext,
    #[serde(default)]
    pub state: ThreadState,
}

impl PersistedThreadSnapshot {
    /// Create one empty persisted snapshot at the provided timestamp.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::PersistedThreadSnapshot;
    ///
    /// let snapshot = PersistedThreadSnapshot::new(Utc::now());
    /// assert!(snapshot.thread.messages.is_empty());
    /// ```
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            thread: ThreadContext::new(now),
            state: ThreadState::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ActiveRequestState {
    external_message_id: Option<String>,
    started_at: DateTime<Utc>,
}

/// Persistence boundary used by live `Thread` handles.
#[async_trait]
pub trait ThreadSnapshotStore: Send + Sync + fmt::Debug {
    /// Persist one thread snapshot using compare-and-swap revision semantics.
    async fn save_thread_snapshot(
        &self,
        locator: &ThreadContextLocator,
        snapshot: &PersistedThreadSnapshot,
        expected_revision: u64,
    ) -> Result<u64>;
}

/// Dedicated runtime container for thread initialization and runtime service access.
#[derive(Clone)]
pub struct ThreadRuntime {
    tool_registry: Arc<ToolRegistry>,
    memory_repository: Arc<MemoryRepository>,
    feature_prompt_rebuilder: Arc<FeaturePromptRebuilder>,
    default_auto_compact_enabled: bool,
}

impl ThreadRuntime {
    /// Build one thread runtime from shared runtime services.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::ThreadRuntime,
    /// };
    /// use std::sync::Arc;
    ///
    /// let tool_registry = Arc::new(ToolRegistry::new());
    /// let memory_repository = Arc::new(MemoryRepository::new("."));
    /// let feature_prompt_rebuilder = Arc::new(FeaturePromptRebuilder::new(
    ///     Arc::clone(&tool_registry),
    ///     Default::default(),
    ///     "",
    /// ));
    ///
    /// let runtime = ThreadRuntime::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     feature_prompt_rebuilder,
    ///     false,
    /// );
    /// assert!(!runtime.default_auto_compact_enabled());
    /// ```
    pub fn new(
        tool_registry: Arc<ToolRegistry>,
        memory_repository: Arc<MemoryRepository>,
        feature_prompt_rebuilder: Arc<FeaturePromptRebuilder>,
        default_auto_compact_enabled: bool,
    ) -> Self {
        Self {
            tool_registry,
            memory_repository,
            feature_prompt_rebuilder,
            default_auto_compact_enabled,
        }
    }

    /// Return the shared tool registry.
    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tool_registry)
    }

    /// Return the shared memory repository.
    pub fn memory_repository(&self) -> Arc<MemoryRepository> {
        Arc::clone(&self.memory_repository)
    }

    /// Return whether auto-compact should be enabled by default for initialized threads.
    pub fn default_auto_compact_enabled(&self) -> bool {
        self.default_auto_compact_enabled
    }

    /// Ensure built-in tools are ready before the thread is served.
    pub async fn ensure_tool_registry_ready(&self) -> Result<()> {
        self.tool_registry.register_builtin_tools().await
    }

    /// Persist one initialized thread prefix before the thread enters normal request handling.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::{Thread, ThreadContextLocator, ThreadRuntime},
    /// };
    /// use std::sync::Arc;
    ///
    /// let tool_registry = Arc::new(ToolRegistry::new());
    /// let memory_repository = Arc::new(MemoryRepository::new("."));
    /// let feature_prompt_rebuilder = Arc::new(FeaturePromptRebuilder::new(
    ///     Arc::clone(&tool_registry),
    ///     Default::default(),
    ///     "system",
    /// ));
    /// let runtime = ThreadRuntime::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     feature_prompt_rebuilder,
    ///     false,
    /// );
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    ///
    /// runtime.initialize_thread(&mut thread).await?;
    /// assert!(thread.is_initialized());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn initialize_thread(&self, thread_context: &mut Thread) -> Result<bool> {
        self.ensure_tool_registry_ready().await?;

        if thread_context.is_initialized() {
            return Ok(false);
        }

        let existing_system_prefix_at = thread_context
            .thread
            .messages
            .iter()
            .find(|message| message.role == ChatMessageRole::System)
            .map(|message| message.created_at);
        if let Some(initialized_at) = existing_system_prefix_at {
            thread_context.mark_initialized(initialized_at).await?;
            info!(
                thread_id = %thread_context.locator.thread_id,
                external_thread_id = %thread_context.locator.external_thread_id,
                initialized_at = %initialized_at,
                "marked existing thread as initialized from persisted system prefix"
            );
            return Ok(true);
        }

        let auto_compact_enabled =
            thread_context.auto_compact_enabled(self.default_auto_compact_enabled);
        let initialized_messages = self
            .feature_prompt_rebuilder
            .build_messages(thread_context, auto_compact_enabled)
            .await?;
        let initialized_at = initialized_messages
            .first()
            .map(|message| message.created_at)
            .unwrap_or_else(Utc::now);
        thread_context
            .initialize_with_messages(initialized_messages, initialized_at)
            .await?;
        info!(
            thread_id = %thread_context.locator.thread_id,
            external_thread_id = %thread_context.locator.external_thread_id,
            initialized_message_count = thread_context
                .thread
                .messages
                .iter()
                .filter(|message| message.role == ChatMessageRole::System)
                .count(),
            "persisted thread initialization prefix"
        );
        Ok(true)
    }
}

impl fmt::Debug for ThreadRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadRuntime")
            .field(
                "default_auto_compact_enabled",
                &self.default_auto_compact_enabled,
            )
            .field("tool_registry", &"Arc<ToolRegistry>")
            .field("memory_repository", &"Arc<MemoryRepository>")
            .field("feature_prompt_rebuilder", &"Arc<FeaturePromptRebuilder>")
            .finish()
    }
}

/// Live thread handle. Persisted state is separated from live request-only state.
#[derive(Clone)]
pub struct Thread {
    pub locator: ThreadContextLocator,
    pub thread: ThreadContext,
    pub state: ThreadState,
    revision: u64,
    active_request: Option<ActiveRequestState>,
    store: Option<Arc<dyn ThreadSnapshotStore>>,
}

impl Thread {
    /// Create one empty live thread handle.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator};
    ///
    /// let thread = Thread::new(
    ///     ThreadContextLocator::new(
    ///         Some("session-1".to_string()),
    ///         "feishu",
    ///         "ou_xxx",
    ///         "thread_ext",
    ///         "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    ///     ),
    ///     Utc::now(),
    /// );
    /// assert_eq!(thread.locator.external_thread_id, "thread_ext");
    /// ```
    pub fn new(locator: ThreadContextLocator, now: DateTime<Utc>) -> Self {
        Self {
            locator,
            thread: ThreadContext::new(now),
            state: ThreadState::default(),
            revision: 0,
            active_request: None,
            store: None,
        }
    }

    /// Rebuild one live thread handle from one persisted snapshot and revision.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{PersistedThreadSnapshot, Thread, ThreadContextLocator};
    ///
    /// let locator =
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal");
    /// let thread = Thread::from_persisted(locator.clone(), PersistedThreadSnapshot::new(Utc::now()), 7);
    /// assert_eq!(thread.locator, locator);
    /// ```
    pub fn from_persisted(
        locator: ThreadContextLocator,
        snapshot: PersistedThreadSnapshot,
        revision: u64,
    ) -> Self {
        Self {
            locator,
            thread: snapshot.thread,
            state: snapshot.state,
            revision,
            active_request: None,
            store: None,
        }
    }

    /// Return the persisted snapshot currently owned by this live handle.
    pub fn persisted_snapshot(&self) -> PersistedThreadSnapshot {
        PersistedThreadSnapshot {
            thread: self.thread.clone(),
            state: self.state.clone(),
        }
    }

    /// Rebind the runtime locator while keeping persisted thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    /// Bind one thread snapshot store to the live handle.
    pub fn bind_store(&mut self, store: Arc<dyn ThreadSnapshotStore>) {
        self.store = Some(store);
    }

    /// Export the thread-owned formal message sequence.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.thread.messages()
    }

    /// Export the compact source message sequence used by runtime compaction.
    pub fn compact_source_messages(&self) -> Vec<ChatMessage> {
        self.thread.messages()
    }

    /// Return whether this thread already owns a persisted initialized prefix.
    pub fn is_initialized(&self) -> bool {
        self.state.lifecycle.initialized
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.state.tools.tool_events.clone()
    }

    /// Return whether the thread currently owns one active request-local state.
    pub fn has_active_request(&self) -> bool {
        self.active_request.is_some()
    }

    /// Return the current active request external message id.
    pub fn current_request_external_message_id(&self) -> Option<String> {
        self.active_request
            .as_ref()
            .and_then(|request| request.external_message_id.clone())
    }

    /// Start one request-local live state without creating any persisted turn structure.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .begin_request(Some("msg_1".to_string()), now)
    ///     .expect("request should start");
    /// assert!(thread.has_active_request());
    /// ```
    pub fn begin_request(
        &mut self,
        external_message_id: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Result<()> {
        if self.active_request.is_some() {
            bail!(
                "thread `{}` already owns one unfinished request",
                self.locator.thread_id
            );
        }

        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            started_at = %started_at,
            "started thread-owned active request"
        );
        self.active_request = Some(ActiveRequestState {
            external_message_id,
            started_at,
        });
        Ok(())
    }

    /// Finish the current request-local live state.
    pub fn finish_request(&mut self, completed_at: DateTime<Utc>, succeeded: bool) -> Result<()> {
        let Some(active_request) = self.active_request.take() else {
            bail!(
                "thread `{}` does not own one active request",
                self.locator.thread_id
            );
        };
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?active_request.external_message_id,
            request_started_at = %active_request.started_at,
            completed_at = %completed_at,
            succeeded,
            "finished thread-owned active request"
        );
        Ok(())
    }

    async fn apply_persisted_mutation<F, R>(
        &mut self,
        mutation_name: &'static str,
        mutate: F,
    ) -> Result<R>
    where
        F: FnOnce(&mut PersistedThreadSnapshot) -> Result<R>,
    {
        let mut next = self.persisted_snapshot();
        let result = mutate(&mut next)?;
        let updated_at = next.thread.updated_at;
        let expected_revision = self.revision;

        if let Some(store) = self.store.as_ref() {
            let new_revision = store
                .save_thread_snapshot(&self.locator, &next, expected_revision)
                .await?;
            self.revision = new_revision;
        }

        self.thread = next.thread;
        self.state = next.state;
        debug!(
            thread_id = %self.locator.thread_id,
            revision = self.revision,
            updated_at = %updated_at,
            mutation = mutation_name,
            "applied thread-owned persisted mutation"
        );
        Ok(result)
    }

    pub(crate) async fn mark_initialized(&mut self, initialized_at: DateTime<Utc>) -> Result<()> {
        self.apply_persisted_mutation("mark_initialized", |snapshot| {
            snapshot.state.lifecycle.initialized = true;
            if snapshot.thread.created_at > initialized_at {
                snapshot.thread.created_at = initialized_at;
            }
            if snapshot.thread.updated_at < initialized_at {
                snapshot.thread.updated_at = initialized_at;
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn initialize_with_messages(
        &mut self,
        initialized_messages: Vec<ChatMessage>,
        initialized_at: DateTime<Utc>,
    ) -> Result<()> {
        self.apply_persisted_mutation("initialize_thread", |snapshot| {
            if !initialized_messages.is_empty() {
                snapshot.thread.messages = initialized_messages;
                snapshot.thread.created_at = initialized_at;
                snapshot.thread.updated_at = initialized_at;
            }
            snapshot.state.lifecycle.initialized = true;
            Ok(())
        })
        .await
    }

    /// Push one formal message into the thread-owned persisted message sequence.
    ///
    /// 调用成功返回时，这条消息已经通过 thread-owned CAS 语义完成落盘。
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .await?;
    ///
    /// assert_eq!(thread.messages()[0].content, "hello");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn push_message(&mut self, message: ChatMessage) -> Result<()> {
        let external_message_id = self.current_request_external_message_id();
        self.apply_persisted_mutation("push_message", |snapshot| {
            if snapshot.thread.created_at > message.created_at {
                snapshot.thread.created_at = message.created_at;
            }
            snapshot.thread.updated_at = message.created_at;
            snapshot.thread.messages.push(message.clone());
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            role = message.role.as_label(),
            tool_call_count = message.tool_calls.len(),
            has_tool_call_id = message.tool_call_id.is_some(),
            "persisted formal thread message"
        );
        Ok(())
    }

    /// Replace the persisted non-system history after one compact rewrite.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .await?;
    /// thread
    ///     .replace_messages_after_compaction(vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
    ///         ChatMessage::new(ChatMessageRole::User, "继续", now),
    ///     ])
    ///     .await?;
    ///
    /// assert_eq!(thread.messages()[0].content, "这是压缩后的上下文");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn replace_messages_after_compaction(
        &mut self,
        compacted_messages: Vec<ChatMessage>,
    ) -> Result<()> {
        let external_message_id = self.current_request_external_message_id();
        self.apply_persisted_mutation("replace_messages_after_compaction", |snapshot| {
            let mut persisted_messages = snapshot
                .thread
                .messages
                .iter()
                .filter(|message| message.role == ChatMessageRole::System)
                .cloned()
                .collect::<Vec<_>>();
            let updated_at = compacted_messages
                .last()
                .map(|message| message.created_at)
                .unwrap_or_else(Utc::now);
            persisted_messages.extend(compacted_messages.clone());
            snapshot.thread.messages = persisted_messages;
            if snapshot.thread.created_at > updated_at {
                snapshot.thread.created_at = updated_at;
            }
            snapshot.thread.updated_at = updated_at;
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            external_message_id = ?external_message_id,
            compacted_message_count = self
                .thread
                .messages
                .iter()
                .filter(|message| message.role != ChatMessageRole::System)
                .count(),
            "rewrote persisted thread non-system history after compaction"
        );
        Ok(())
    }

    /// Append one persisted thread tool event immediately.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator, ThreadToolEvent, ThreadToolEventKind};
    ///
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// thread
    ///     .append_tool_event(ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, Utc::now()))
    ///     .await?;
    /// assert_eq!(thread.load_tool_events().len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn append_tool_event(&mut self, event: ThreadToolEvent) -> Result<()> {
        let event_kind = event.kind.clone();
        let recorded_at = event.recorded_at;
        self.apply_persisted_mutation("append_tool_event", |snapshot| {
            snapshot.state.tools.tool_events.push(event);
            if snapshot.thread.updated_at < recorded_at {
                snapshot.thread.updated_at = recorded_at;
            }
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            event_kind = ?event_kind,
            "persisted thread tool audit event"
        );
        Ok(())
    }

    /// Replace the thread's loaded toolset state with a normalized snapshot in memory only.
    pub fn replace_loaded_toolsets(&mut self, loaded_toolsets: Vec<String>) {
        self.state.tools.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
    }

    /// Persist one thread-scoped auto-compact override immediately.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::thread::{Thread, ThreadContextLocator};
    ///
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// thread.persist_auto_compact_override(Some(true)).await?;
    /// assert!(thread.auto_compact_enabled(false));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn persist_auto_compact_override(&mut self, enabled: Option<bool>) -> Result<()> {
        let updated_at = Utc::now();
        self.apply_persisted_mutation("persist_auto_compact_override", |snapshot| {
            snapshot.state.features.auto_compact_override = enabled;
            snapshot.thread.updated_at = updated_at;
            Ok(())
        })
        .await?;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            auto_compact_override = ?enabled,
            "persisted thread auto-compact override"
        );
        Ok(())
    }

    /// Mark one toolset as loaded for the current thread and persist the change atomically.
    pub async fn load_toolset(&mut self, toolset_name: &str) -> Result<bool> {
        let toolset_name = toolset_name.trim().to_string();
        if toolset_name.is_empty() {
            return Ok(false);
        }

        let inserted = self
            .apply_persisted_mutation("load_toolset", |snapshot| {
                let already_loaded = snapshot
                    .state
                    .tools
                    .loaded_toolsets
                    .binary_search_by(|candidate| candidate.as_str().cmp(&toolset_name))
                    .is_ok();
                if !already_loaded {
                    snapshot
                        .state
                        .tools
                        .loaded_toolsets
                        .push(toolset_name.clone());
                    snapshot.state.tools.loaded_toolsets.sort();
                    snapshot.state.tools.loaded_toolsets.dedup();
                    snapshot.thread.updated_at = Utc::now();
                }
                Ok(!already_loaded)
            })
            .await?;
        info!(
            thread_id = %self.locator.thread_id,
            toolset_name = %toolset_name,
            inserted,
            "updated persisted thread loaded toolsets"
        );
        Ok(inserted)
    }

    /// Mark one toolset as unloaded for the current thread and persist the change atomically.
    pub async fn unload_toolset(&mut self, toolset_name: &str) -> Result<bool> {
        let toolset_name = toolset_name.trim().to_string();
        if toolset_name.is_empty() {
            return Ok(false);
        }

        let removed = self
            .apply_persisted_mutation("unload_toolset", |snapshot| {
                let original_len = snapshot.state.tools.loaded_toolsets.len();
                snapshot
                    .state
                    .tools
                    .loaded_toolsets
                    .retain(|candidate| candidate != &toolset_name);
                let removed = original_len != snapshot.state.tools.loaded_toolsets.len();
                if removed {
                    snapshot.thread.updated_at = Utc::now();
                }
                Ok(removed)
            })
            .await?;
        info!(
            thread_id = %self.locator.thread_id,
            toolset_name = %toolset_name,
            removed,
            "updated persisted thread loaded toolsets"
        );
        Ok(removed)
    }

    pub(crate) async fn toolset_catalog_prompt_with_registry(
        &self,
        tool_registry: &ToolRegistry,
    ) -> Option<String> {
        tool_registry
            .render_toolset_catalog_prompt(&self.load_toolsets())
            .await
    }

    pub(crate) async fn visible_tools_with_registry(
        &self,
        tool_registry: &ToolRegistry,
        compact_visible: bool,
    ) -> Result<Vec<ToolDefinition>> {
        let mut definitions = tool_registry.always_visible_definitions().await;
        definitions.push(crate::agent::tool::load_toolset_definition());
        definitions.push(crate::agent::tool::unload_toolset_definition());

        for toolset_name in self.load_toolsets() {
            definitions.extend(tool_registry.toolset_definitions(&toolset_name).await?);
        }

        if compact_visible {
            definitions.push(crate::agent::tool::compact_tool_definition());
        }

        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(definitions)
    }

    async fn load_toolset_via_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        toolset_name: &str,
    ) -> Result<bool> {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() {
            bail!("load_toolset requires a non-empty tool name");
        }

        tool_registry.toolset_definitions(toolset_name).await?;
        self.load_toolset(toolset_name).await
    }

    async fn unload_toolset_via_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        toolset_name: &str,
    ) -> Result<bool> {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() {
            bail!("unload_toolset requires a non-empty tool name");
        }

        let thread_id = self.locator.thread_id.clone();
        let is_loaded = self
            .load_toolsets()
            .into_iter()
            .any(|loaded_name| loaded_name == toolset_name);
        if is_loaded && let Some(runtime) = tool_registry.toolset_runtime(toolset_name).await? {
            runtime.on_unload(&thread_id).await?;
        }

        self.unload_toolset(toolset_name).await
    }

    pub(crate) async fn call_tool_with_registry(
        &mut self,
        tool_registry: &ToolRegistry,
        request: ToolCallRequest,
    ) -> Result<ToolCallResult> {
        let thread_id = self.locator.thread_id.clone();
        let tool_name = request.name.clone();
        let loaded_toolset_count = self.load_toolsets().len();
        let argument_field_count = request
            .arguments
            .as_object()
            .map(|arguments| arguments.len())
            .unwrap_or_default();
        let started_at = std::time::Instant::now();
        tracing::debug!(
            thread_id = %thread_id,
            tool_name = %tool_name,
            loaded_toolset_count,
            argument_field_count,
            "starting thread-owned tool call"
        );

        let result = match request.name.as_str() {
            "compact" => bail!("tool `compact` must be handled by the agent loop compact runtime"),
            "load_toolset" => {
                let toolset_name = request
                    .arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("load_toolset requires a non-empty tool name")
                    })?;
                let inserted = self
                    .load_toolset_via_registry(tool_registry, toolset_name)
                    .await?;
                let loaded_toolsets = self.load_toolsets();
                Ok(ToolCallResult {
                    content: if inserted {
                        format!("Toolset `{toolset_name}` loaded for the current thread.")
                    } else {
                        format!(
                            "Toolset `{toolset_name}` was already loaded for the current thread."
                        )
                    },
                    metadata: json!({
                        "event_kind": "load_toolset",
                        "toolset": toolset_name,
                        "loaded_toolsets": loaded_toolsets,
                        "already_loaded": !inserted,
                        "approval_required": false,
                        "policy_extension_point": true,
                    }),
                    is_error: false,
                })
            }
            "unload_toolset" => {
                let toolset_name = request
                    .arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("unload_toolset requires a non-empty tool name")
                    })?;
                let removed = self
                    .unload_toolset_via_registry(tool_registry, toolset_name)
                    .await?;
                let loaded_toolsets = self.load_toolsets();
                Ok(ToolCallResult {
                    content: if removed {
                        format!("Toolset `{toolset_name}` unloaded for the current thread.")
                    } else {
                        format!("Toolset `{toolset_name}` was not loaded for the current thread.")
                    },
                    metadata: json!({
                        "event_kind": "unload_toolset",
                        "toolset": toolset_name,
                        "loaded_toolsets": loaded_toolsets,
                        "was_loaded": removed,
                        "approval_required": false,
                        "policy_extension_point": true,
                    }),
                    is_error: false,
                })
            }
            _ => {
                let context = crate::agent::ToolCallContext::for_thread(thread_id.clone());
                if let Some(handler) = tool_registry.always_visible_handler(&request.name).await {
                    handler.call_with_context(context, request).await
                } else {
                    let mut resolved_handler = None;
                    for toolset_name in self.load_toolsets() {
                        if let Some(handler) = tool_registry
                            .toolset_handler(&toolset_name, &request.name)
                            .await?
                        {
                            resolved_handler = Some(handler);
                            break;
                        }
                    }
                    let Some(handler) = resolved_handler else {
                        bail!(
                            "tool `{}` is not registered for thread `{}`",
                            request.name,
                            thread_id
                        );
                    };
                    handler
                        .call_with_context(
                            crate::agent::ToolCallContext::for_thread(thread_id.clone()),
                            request,
                        )
                        .await
                }
            }
        };

        match &result {
            Ok(tool_result) => tracing::debug!(
                thread_id = %thread_id,
                tool_name = %tool_name,
                loaded_toolset_count,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                is_error = tool_result.is_error,
                event_kind = ?tool_result
                    .metadata
                    .get("event_kind")
                    .and_then(|value| value.as_str()),
                "completed thread-owned tool call"
            ),
            Err(error) => tracing::debug!(
                thread_id = %thread_id,
                tool_name = %tool_name,
                loaded_toolset_count,
                argument_field_count,
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "thread-owned tool call failed"
            ),
        }
        result
    }

    /// Clear the current thread back to one empty initial state in memory only.
    pub fn clear_to_initial_state(&mut self, now: DateTime<Utc>) {
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            "clearing thread back to initial state"
        );
        self.thread = ThreadContext::new(now);
        self.state = ThreadState::default();
        self.active_request = None;
    }

    /// Persist one reset that clears the thread back to one empty initial state.
    pub async fn reset_to_initial_state(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.apply_persisted_mutation("reset_to_initial_state", |snapshot| {
            snapshot.thread = ThreadContext::new(now);
            snapshot.state = ThreadState::default();
            Ok(())
        })
        .await?;
        self.active_request = None;
        Ok(())
    }

    /// Replace the active thread snapshot while keeping the current locator.
    #[doc(hidden)]
    pub fn overwrite_active_history(&mut self, replacement: &Thread) {
        self.thread = replacement.thread.clone();
        self.state = replacement.state.clone();
    }

    /// Return the effective auto-compact state for this thread.
    pub fn auto_compact_enabled(&self, default_enabled: bool) -> bool {
        self.state
            .features
            .auto_compact_override
            .unwrap_or(default_enabled)
    }

    /// Update the auto-compact override for the current thread in memory only.
    pub fn set_auto_compact_override(&mut self, enabled: Option<bool>) {
        self.state.features.auto_compact_override = enabled;
    }

    /// Enable thread-scoped auto-compact for the current thread.
    pub fn enable_auto_compact(&mut self) {
        self.set_auto_compact_override(Some(true));
    }

    /// Disable thread-scoped auto-compact and fall back to the static auto-compact default.
    pub fn disable_auto_compact(&mut self) {
        self.set_auto_compact_override(Some(false));
    }
}

impl Deref for Thread {
    type Target = ThreadContext;

    fn deref(&self) -> &Self::Target {
        &self.thread
    }
}

impl DerefMut for Thread {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.thread
    }
}

impl fmt::Debug for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thread")
            .field("locator", &self.locator)
            .field("thread", &self.thread)
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("active_request", &self.active_request)
            .finish()
    }
}

impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.locator == other.locator
            && self.thread == other.thread
            && self.state == other.state
            && self.revision == other.revision
            && self.active_request == other.active_request
    }
}
