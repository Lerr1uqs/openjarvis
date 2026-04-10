//! Thread aggregate and persisted thread state model.

use crate::agent::{
    FeaturePromptRebuilder, MemoryRepository, ToolCallRequest, ToolCallResult, ToolDefinition,
    ToolRegistry,
};
use crate::context::{ChatMessage, ChatMessageRole};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tracing::{error, info};
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

/// Message visibility scope owned by the current thread turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadMessageScope {
    PersistedHistory,
    RequestOnly,
}

/// One unified thread message write request routed through `Thread::push_message(...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadMessageInput {
    pub message: ChatMessage,
    pub scope: ThreadMessageScope,
}

impl ThreadMessageInput {
    /// Wrap one message as a request-only input that only lives in the current request view.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ThreadMessageInput, ThreadMessageScope},
    /// };
    ///
    /// let input = ThreadMessageInput::request_only(ChatMessage::new(
    ///     ChatMessageRole::System,
    ///     "runtime memory",
    ///     Utc::now(),
    /// ));
    /// assert_eq!(input.scope, ThreadMessageScope::RequestOnly);
    /// ```
    pub fn request_only(message: ChatMessage) -> Self {
        Self {
            message,
            scope: ThreadMessageScope::RequestOnly,
        }
    }
}

impl From<ChatMessage> for ThreadMessageInput {
    fn from(message: ChatMessage) -> Self {
        Self {
            message,
            scope: ThreadMessageScope::PersistedHistory,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Uuid>,
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
    /// Create one structured thread tool event without a bound turn id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{ThreadToolEvent, ThreadToolEventKind};
    ///
    /// let event = ThreadToolEvent::new(ThreadToolEventKind::ExecuteTool, Utc::now());
    /// assert!(event.turn_id.is_none());
    /// ```
    pub fn new(kind: ThreadToolEventKind, recorded_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            turn_id: None,
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

    /// Attach the stored turn id after the owning turn has been created.
    pub fn with_turn_id(mut self, turn_id: Uuid) -> Self {
        self.turn_id = Some(turn_id);
        self
    }
}

/// User-visible event buffered inside one thread-owned turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThreadTurnEventKind {
    TextOutput,
    ToolCall,
    ToolResult,
    Compact,
}

/// One thread-owned dispatch event that only becomes externally visible after turn finalization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadTurnEvent {
    pub kind: ThreadTurnEventKind,
    pub content: String,
    #[serde(default = "default_tool_event_metadata")]
    pub metadata: Value,
}

/// Finalized status for one thread-owned turn result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThreadFinalizedTurnStatus {
    Succeeded,
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq)]
struct ThreadCurrentTurn {
    turn_id: Uuid,
    external_message_id: Option<String>,
    started_at: DateTime<Utc>,
    buffered_events: Vec<ThreadTurnEvent>,
    tool_events: Vec<ThreadToolEvent>,
    request_only_messages: Vec<ChatMessage>,
}

impl ThreadCurrentTurn {
    fn new(external_message_id: Option<String>, started_at: DateTime<Utc>) -> Self {
        Self {
            turn_id: Uuid::new_v4(),
            external_message_id,
            started_at,
            buffered_events: Vec::new(),
            tool_events: Vec::new(),
            request_only_messages: Vec::new(),
        }
    }
}

/// One finalized thread-owned turn that binds the event batch and the persisted snapshot edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadFinalizedTurn {
    pub turn_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_message_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub reply: String,
    pub status: ThreadFinalizedTurnStatus,
    pub events: Vec<ThreadTurnEvent>,
    pub snapshot: Thread,
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
    /// ```rust
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
    /// ```rust
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
    /// ```
    pub fn thread_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.user_id, self.channel, self.external_thread_id
        )
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_context_initialized_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ThreadContext {
    /// Create one empty persisted thread context.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            messages: Vec::new(),
            request_context_initialized_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Export all persisted messages currently owned by the thread.
    pub fn messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }
}

/// Runtime attachment bundle injected into one live thread before request handling starts.
#[derive(Clone)]
pub struct ThreadRuntimeAttachment {
    tool_registry: Arc<ToolRegistry>,
    memory_repository: Arc<MemoryRepository>,
    feature_prompt_rebuilder: Arc<FeaturePromptRebuilder>,
    default_auto_compact_enabled: bool,
}

impl ThreadRuntimeAttachment {
    /// Build one thread runtime attachment from shared runtime services.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::ThreadRuntimeAttachment,
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
    /// let attachment = ThreadRuntimeAttachment::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     feature_prompt_rebuilder,
    ///     false,
    /// );
    /// assert!(!attachment.default_auto_compact_enabled());
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

    /// Return whether auto-compact should be enabled by default for attached threads.
    pub fn default_auto_compact_enabled(&self) -> bool {
        self.default_auto_compact_enabled
    }

    async fn ensure_tool_registry_ready(&self) -> Result<()> {
        self.tool_registry.register_builtin_tools().await
    }
}

impl fmt::Debug for ThreadRuntimeAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadRuntimeAttachment")
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

/// Unified persisted thread aggregate shared by session, command, and agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub locator: ThreadContextLocator,
    pub thread: ThreadContext,
    #[serde(default)]
    pub state: ThreadState,
    #[serde(default, skip_serializing, skip_deserializing)]
    revision: u64,
    #[serde(default, skip_serializing, skip_deserializing)]
    pending_tool_events: Vec<ThreadToolEvent>,
    #[serde(default, skip_serializing, skip_deserializing)]
    current_turn: Option<ThreadCurrentTurn>,
    #[serde(default, skip_serializing, skip_deserializing)]
    runtime_attachment: Option<ThreadRuntimeAttachment>,
}

impl Thread {
    /// Create one empty thread aggregate.
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
            pending_tool_events: Vec::new(),
            current_turn: None,
            runtime_attachment: None,
        }
    }

    /// Rebind the runtime locator while keeping persisted thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    /// Attach the shared runtime services required by this thread during live request handling.
    ///
    /// # 示例
    /// ```rust,no_run
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::{Thread, ThreadContextLocator, ThreadRuntimeAttachment},
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
    /// let attachment = ThreadRuntimeAttachment::new(
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
    /// thread.attach_runtime(attachment);
    /// assert!(thread.has_runtime());
    /// ```
    pub fn attach_runtime(&mut self, attachment: ThreadRuntimeAttachment) {
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            "attached runtime services to thread"
        );
        self.runtime_attachment = Some(attachment);
    }

    /// Return whether the current thread already has a live runtime attachment.
    pub fn has_runtime(&self) -> bool {
        self.runtime_attachment.is_some()
    }

    pub(crate) fn detach_runtime(&mut self) {
        self.runtime_attachment = None;
    }

    fn runtime_attachment(&self) -> Result<&ThreadRuntimeAttachment> {
        self.runtime_attachment.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "thread `{}` does not have attached runtime services",
                self.locator.thread_id
            )
        })
    }

    fn backfill_initialized_at_from_system_prefix(&mut self) -> Option<DateTime<Utc>> {
        let initialized_at = self
            .thread
            .messages
            .iter()
            .find(|message| message.role == ChatMessageRole::System)
            .map(|message| message.created_at)?;
        self.thread.request_context_initialized_at = Some(initialized_at);
        Some(initialized_at)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    /// Export the thread-owned formal message sequence.
    ///
    /// # 示例
    /// ```rust
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
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should append");
    ///
    /// assert_eq!(thread.messages()[0].role, ChatMessageRole::User);
    /// assert_eq!(thread.messages()[0].content, "hello");
    /// ```
    pub fn messages(&self) -> Vec<ChatMessage> {
        let mut messages = self.thread.messages();
        if let Some(current_turn) = &self.current_turn {
            messages.extend(current_turn.request_only_messages.clone());
        }
        messages
    }

    /// Export the compact source message sequence without request-only runtime messages.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator, ThreadMessageInput},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread.begin_turn(Some("msg_1".to_string()), now).expect("turn should start");
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("persisted message should append");
    /// thread
    ///     .push_message(ThreadMessageInput::request_only(ChatMessage::new(
    ///         ChatMessageRole::System,
    ///         "runtime memory",
    ///         now,
    ///     )))
    ///     .expect("request-only message should append");
    ///
    /// assert_eq!(thread.messages().len(), 2);
    /// assert_eq!(thread.compact_source_messages().len(), 1);
    /// ```
    pub fn compact_source_messages(&self) -> Vec<ChatMessage> {
        self.thread.messages()
    }

    /// Return whether this thread already owns a stable initialized request-context snapshot.
    pub fn is_initialized(&self) -> bool {
        self.thread.request_context_initialized_at.is_some()
    }

    /// Ensure the current thread has finished stable request-context initialization.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::{Thread, ThreadContextLocator, ThreadRuntimeAttachment},
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
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// thread.attach_runtime(ThreadRuntimeAttachment::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     feature_prompt_rebuilder,
    ///     false,
    /// ));
    ///
    /// let _ = thread.ensure_initialized().await?;
    /// assert!(thread.is_initialized());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn ensure_initialized(&mut self) -> Result<bool> {
        let runtime_attachment = self.runtime_attachment()?.clone();
        runtime_attachment.ensure_tool_registry_ready().await?;

        if self.is_initialized() {
            return Ok(false);
        }

        if let Some(initialized_at) = self.backfill_initialized_at_from_system_prefix() {
            info!(
                thread_id = %self.locator.thread_id,
                external_thread_id = %self.locator.external_thread_id,
                initialized_at = %initialized_at,
                "backfilled thread initialization marker from existing system prefix"
            );
            return Ok(true);
        }

        let auto_compact_enabled =
            self.auto_compact_enabled(runtime_attachment.default_auto_compact_enabled());
        let initialized_messages = runtime_attachment
            .feature_prompt_rebuilder
            .build_messages(self, auto_compact_enabled)
            .await?;
        let initialized_at = initialized_messages
            .first()
            .map(|message| message.created_at)
            .unwrap_or_else(Utc::now);
        if !initialized_messages.is_empty() {
            self.replace_messages(initialized_messages, initialized_at);
        }
        self.thread.request_context_initialized_at = Some(initialized_at);
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            initialized_message_count = self
                .thread
                .messages
                .iter()
                .filter(|message| message.role == ChatMessageRole::System)
                .count(),
            "thread ensured initialized request-context snapshot"
        );
        Ok(true)
    }

    /// Return the memory repository attached to the current live thread runtime.
    pub fn memory_repository(&self) -> Result<Arc<MemoryRepository>> {
        Ok(Arc::clone(&self.runtime_attachment()?.memory_repository))
    }

    /// Refresh request-time memory messages for the current turn.
    ///
    /// 当前 memory 语义仍然遵循“渐进式披露”原则:
    /// repository 不会根据关键词自动把正文塞进请求；因此这个入口目前只负责把
    /// request-time memory 的 ownership 固定在 `Thread`，并在未来真正需要注入时通过
    /// `push_message(...)` 写入 request-only working set。
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo() -> anyhow::Result<()> {
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     agent::{FeaturePromptRebuilder, MemoryRepository, ToolRegistry},
    ///     thread::{Thread, ThreadContextLocator, ThreadRuntimeAttachment},
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
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// thread.attach_runtime(ThreadRuntimeAttachment::new(
    ///     tool_registry,
    ///     memory_repository,
    ///     feature_prompt_rebuilder,
    ///     false,
    /// ));
    /// thread.begin_turn(Some("msg_1".to_string()), Utc::now())?;
    ///
    /// let injected = thread.refresh_request_time_memory().await?;
    /// assert_eq!(injected, 0);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn refresh_request_time_memory(&mut self) -> Result<usize> {
        let memory_repository = self.memory_repository()?;
        let memory_messages = memory_repository.build_request_time_messages(self)?;
        let current_turn = self.current_turn_mut()?;
        current_turn.request_only_messages.clear();
        let turn_id = current_turn.turn_id;
        let memory_root = memory_repository.memory_root();
        let mut injected_count = 0usize;
        for memory_message in memory_messages {
            self.push_message(ThreadMessageInput::request_only(memory_message))?;
            injected_count += 1;
        }
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %turn_id,
            request_time_memory_count = injected_count,
            memory_root = %memory_root.display(),
            "refreshed thread-owned request-time memory messages"
        );
        Ok(injected_count)
    }

    /// Replace the current persisted message sequence.
    pub(crate) fn replace_messages(
        &mut self,
        replacement: Vec<ChatMessage>,
        updated_at: DateTime<Utc>,
    ) {
        self.thread.messages = replacement;
        self.thread.updated_at = updated_at;
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.state.tools.tool_events.clone()
    }

    /// Return the current pending tool events that still need to be bound to one stored turn.
    pub fn pending_tool_events(&self) -> &[ThreadToolEvent] {
        self.current_turn
            .as_ref()
            .map(|turn| turn.tool_events.as_slice())
            .unwrap_or(&self.pending_tool_events)
    }

    /// Record one thread-scoped tool event on the current runtime context.
    pub fn record_tool_event(&mut self, event: ThreadToolEvent) {
        if let Some(current_turn) = self.current_turn.as_mut() {
            current_turn.tool_events.push(event);
            return;
        }

        self.pending_tool_events.push(event);
    }

    /// Return the current buffered turn events.
    pub fn current_turn_events(&self) -> Vec<ThreadTurnEvent> {
        self.current_turn
            .as_ref()
            .map(|turn| turn.buffered_events.clone())
            .unwrap_or_default()
    }

    /// Start one thread-owned turn from the incoming external message id and user message.
    ///
    /// # 示例
    /// ```rust
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
    /// let turn_id = thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should append");
    ///
    /// assert_eq!(thread.pending_tool_events().len(), 0);
    /// assert_ne!(turn_id, uuid::Uuid::nil());
    /// ```
    pub fn begin_turn(
        &mut self,
        external_message_id: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Result<Uuid> {
        Ok(self.open_turn(external_message_id, started_at)?.turn_id)
    }

    /// Return the active thread-owned turn id.
    pub fn current_turn_id(&self) -> Option<Uuid> {
        self.current_turn.as_ref().map(|turn| turn.turn_id)
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

    /// Return the current thread-scoped visible tools by delegating to the attached runtime.
    pub async fn visible_tools(&self, compact_visible: bool) -> Result<Vec<ToolDefinition>> {
        let runtime_attachment = self.runtime_attachment()?.clone();
        runtime_attachment.ensure_tool_registry_ready().await?;
        self.visible_tools_with_registry(&runtime_attachment.tool_registry, compact_visible)
            .await
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
        Ok(self.load_toolset(toolset_name))
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

        Ok(self.unload_toolset(toolset_name))
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

    /// Execute one thread-scoped tool call through the attached global tool registry.
    pub async fn call_tool(&mut self, request: ToolCallRequest) -> Result<ToolCallResult> {
        let runtime_attachment = self.runtime_attachment()?.clone();
        runtime_attachment.ensure_tool_registry_ready().await?;
        self.call_tool_with_registry(&runtime_attachment.tool_registry, request)
            .await
    }

    /// Push one formal message into the thread-owned message sequence.
    ///
    /// # 示例
    /// ```rust
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
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .push_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should append");
    ///
    /// assert_eq!(thread.messages()[0].content, "hello");
    /// ```
    pub fn push_message(&mut self, input: impl Into<ThreadMessageInput>) -> Result<()> {
        let input = input.into();
        let message = input.message;
        let turn_id = self.current_turn_mut()?.turn_id;
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %turn_id,
            role = message.role.as_label(),
            scope = ?input.scope,
            tool_call_count = message.tool_calls.len(),
            has_tool_call_id = message.tool_call_id.is_some(),
            "appended formal thread message"
        );
        match input.scope {
            ThreadMessageScope::PersistedHistory => {
                if self.thread.created_at > message.created_at {
                    self.thread.created_at = message.created_at;
                }
                self.thread.updated_at = message.created_at;
                self.thread.messages.push(message);
            }
            ThreadMessageScope::RequestOnly => {
                self.current_turn_mut()?.request_only_messages.push(message);
            }
        }
        Ok(())
    }

    /// Compatibility wrapper for older call sites still using the previous message API name.
    pub fn append_message(&mut self, message: ChatMessage) -> Result<()> {
        self.push_message(message)
    }

    /// Buffer one user-visible turn event that will only be dispatched after finalization.
    pub fn buffer_turn_event(&mut self, event: ThreadTurnEvent) -> Result<()> {
        let current_turn = self.current_turn_mut()?;
        current_turn.buffered_events.push(event);
        Ok(())
    }

    /// Replace the persisted non-system history after one compact rewrite.
    ///
    /// # 示例
    /// ```rust
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
    /// thread.begin_turn(Some("msg_1".to_string()), now).expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("message should append");
    /// thread
    ///     .replace_messages_after_compaction(vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now),
    ///         ChatMessage::new(ChatMessageRole::User, "继续", now),
    ///     ])
    ///     .expect("compaction should rewrite history");
    ///
    /// assert_eq!(thread.messages()[0].content, "这是压缩后的上下文");
    /// ```
    pub fn replace_messages_after_compaction(
        &mut self,
        compacted_messages: Vec<ChatMessage>,
    ) -> Result<()> {
        let turn_id = self.current_turn_mut()?.turn_id;
        let mut persisted_messages = self
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
        persisted_messages.extend(compacted_messages);
        self.replace_messages(persisted_messages, updated_at);
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %turn_id,
            compacted_message_count = self
                .thread
                .messages
                .iter()
                .filter(|message| message.role != ChatMessageRole::System)
                .count(),
            "rewrote thread non-system history after compaction"
        );
        Ok(())
    }

    /// Finalize the current turn as one successful thread snapshot and event batch.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator, ThreadFinalizedTurnStatus, ThreadTurnEvent, ThreadTurnEventKind},
    /// };
    /// use serde_json::json;
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should append");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::Assistant, "world", now))
    ///     .expect("assistant reply should append");
    /// thread
    ///     .buffer_turn_event(ThreadTurnEvent {
    ///         kind: ThreadTurnEventKind::TextOutput,
    ///         content: "world".to_string(),
    ///         metadata: json!({ "is_final": true }),
    ///     })
    ///     .expect("event should buffer");
    ///
    /// let finalized = thread
    ///     .finalize_turn_success("world", now)
    ///     .expect("turn should finalize");
    ///
    /// assert!(matches!(finalized.status, ThreadFinalizedTurnStatus::Succeeded));
    /// assert_eq!(finalized.snapshot.messages().len(), 2);
    /// ```
    pub fn finalize_turn_success(
        &mut self,
        reply: impl Into<String>,
        completed_at: DateTime<Utc>,
    ) -> Result<ThreadFinalizedTurn> {
        self.finalize_turn(
            ThreadFinalizedTurnStatus::Succeeded,
            reply.into(),
            completed_at,
        )
    }

    /// Finalize the current turn as one failed thread snapshot and matching event batch.
    ///
    /// 异常失败不会回滚已经 append 的正式消息；若当前 turn 还没有任何 turn 事件，
    /// 会补一条 failure event 作为审计输出。
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{Thread, ThreadContextLocator, ThreadFinalizedTurnStatus},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = Thread::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// thread.begin_turn(Some("msg_0".to_string()), now).expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::Assistant, "persisted", now))
    ///     .expect("persisted message should append");
    /// thread
    ///     .finalize_turn_success("persisted", now)
    ///     .expect("seed turn should finalize");
    /// thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .append_message(ChatMessage::new(ChatMessageRole::Assistant, "partial reply", now))
    ///     .expect("partial reply should append");
    ///
    /// let finalized = thread
    ///     .finalize_turn_failure("network error", now)
    ///     .expect("failed turn should finalize");
    ///
    /// assert!(matches!(
    ///     finalized.status,
    ///     ThreadFinalizedTurnStatus::Failed { .. }
    /// ));
    /// assert_eq!(finalized.snapshot.messages().len(), 2);
    /// assert_eq!(finalized.snapshot.messages()[1].content, "partial reply");
    /// ```
    pub fn finalize_turn_failure(
        &mut self,
        error: impl Into<String>,
        completed_at: DateTime<Utc>,
    ) -> Result<ThreadFinalizedTurn> {
        let error = error.into();
        let Some(current_turn) = self.current_turn.take() else {
            bail!(
                "thread `{}` does not own one active turn to finalize",
                self.locator.thread_id
            );
        };
        let failure_reply = format!("[openjarvis][agent_error] {error}");
        error!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %current_turn.turn_id,
            dropped_event_count = current_turn.buffered_events.len(),
            dropped_tool_event_count = current_turn.tool_events.len(),
            dropped_request_only_message_count = current_turn.request_only_messages.len(),
            error = %error,
            message_count = self.messages().len(),
            "thread-owned turn failed without rolling back committed messages"
        );

        self.thread.updated_at = completed_at;
        if self.thread.created_at > current_turn.started_at {
            self.thread.created_at = current_turn.started_at;
        }
        self.state.tools.tool_events.extend(
            current_turn
                .tool_events
                .into_iter()
                .map(|event| event.with_turn_id(current_turn.turn_id)),
        );
        let mut events = current_turn.buffered_events;
        if events.is_empty() {
            events.push(ThreadTurnEvent {
                kind: ThreadTurnEventKind::TextOutput,
                content: failure_reply.clone(),
                metadata: json!({
                    "source": "turn_failure",
                    "is_final": true,
                    "is_error": true,
                }),
            });
        }
        Ok(ThreadFinalizedTurn {
            turn_id: current_turn.turn_id,
            external_message_id: current_turn.external_message_id,
            started_at: current_turn.started_at,
            completed_at,
            reply: failure_reply.clone(),
            status: ThreadFinalizedTurnStatus::Failed { error },
            events,
            snapshot: self.clone(),
        })
    }

    /// Replace the thread's loaded toolset state with a normalized snapshot.
    pub fn replace_loaded_toolsets(&mut self, loaded_toolsets: Vec<String>) {
        self.state.tools.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
    }

    /// Mark one toolset as loaded for the current thread.
    pub fn load_toolset(&mut self, toolset_name: &str) -> bool {
        let toolset_name = toolset_name.trim();
        if toolset_name.is_empty() {
            return false;
        }

        let inserted = self
            .state
            .tools
            .loaded_toolsets
            .binary_search_by(|candidate| candidate.as_str().cmp(toolset_name))
            .is_err();
        if inserted {
            self.state
                .tools
                .loaded_toolsets
                .push(toolset_name.to_string());
            self.state.tools.loaded_toolsets.sort();
            self.state.tools.loaded_toolsets.dedup();
        }
        inserted
    }

    /// Mark one toolset as unloaded for the current thread.
    pub fn unload_toolset(&mut self, toolset_name: &str) -> bool {
        let original_len = self.state.tools.loaded_toolsets.len();
        self.state
            .tools
            .loaded_toolsets
            .retain(|candidate| candidate != toolset_name);
        original_len != self.state.tools.loaded_toolsets.len()
    }

    fn open_turn(
        &mut self,
        external_message_id: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Result<&mut ThreadCurrentTurn> {
        if self.current_turn.is_some() {
            bail!(
                "thread `{}` already owns one unfinished turn",
                self.locator.thread_id
            );
        }

        let mut current_turn = ThreadCurrentTurn::new(external_message_id, started_at);
        current_turn
            .tool_events
            .append(&mut self.pending_tool_events);
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %current_turn.turn_id,
            "started thread-owned turn"
        );
        self.current_turn = Some(current_turn);
        Ok(self
            .current_turn
            .as_mut()
            .expect("current turn should exist immediately after open_turn"))
    }

    fn current_turn_mut(&mut self) -> Result<&mut ThreadCurrentTurn> {
        self.current_turn.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "thread `{}` does not own one active turn",
                self.locator.thread_id
            )
        })
    }

    fn finalize_turn(
        &mut self,
        status: ThreadFinalizedTurnStatus,
        reply: String,
        completed_at: DateTime<Utc>,
    ) -> Result<ThreadFinalizedTurn> {
        let Some(current_turn) = self.current_turn.take() else {
            bail!(
                "thread `{}` does not own one active turn to finalize",
                self.locator.thread_id
            );
        };

        self.thread.updated_at = completed_at;
        if self.thread.created_at > current_turn.started_at {
            self.thread.created_at = current_turn.started_at;
        }

        self.state.tools.tool_events.extend(
            current_turn
                .tool_events
                .into_iter()
                .map(|event| event.with_turn_id(current_turn.turn_id)),
        );
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            turn_id = %current_turn.turn_id,
            event_count = current_turn.buffered_events.len(),
            request_only_message_count = current_turn.request_only_messages.len(),
            is_error = matches!(status, ThreadFinalizedTurnStatus::Failed { .. }),
            "finalized thread-owned turn"
        );

        Ok(ThreadFinalizedTurn {
            turn_id: current_turn.turn_id,
            external_message_id: current_turn.external_message_id,
            started_at: current_turn.started_at,
            completed_at,
            reply,
            status,
            events: current_turn.buffered_events,
            snapshot: self.clone(),
        })
    }

    /// Replace the active thread snapshot while keeping the current locator.
    pub fn overwrite_active_history(&mut self, replacement: &Thread) {
        self.thread = replacement.thread.clone();
        self.state = replacement.state.clone();
        self.pending_tool_events = replacement.pending_tool_events.clone();
        self.current_turn = replacement.current_turn.clone();
    }

    /// Clear the current thread back to one empty initial state.
    pub fn clear_to_initial_state(&mut self, now: DateTime<Utc>) {
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            "clearing thread back to initial state"
        );
        self.thread = ThreadContext::new(now);
        self.state = ThreadState::default();
        self.pending_tool_events.clear();
        self.current_turn = None;
    }

    /// Return the effective auto-compact state for this thread.
    pub fn auto_compact_enabled(&self, default_enabled: bool) -> bool {
        self.state
            .features
            .auto_compact_override
            .unwrap_or(default_enabled)
    }

    /// Update the auto-compact override for the current thread.
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

impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.locator == other.locator
            && self.thread == other.thread
            && self.state == other.state
            && self.revision == other.revision
            && self.pending_tool_events == other.pending_tool_events
            && self.current_turn == other.current_turn
    }
}
