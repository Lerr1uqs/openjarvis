//! Thread aggregate and persisted thread state model.

use crate::context::{ChatMessage, ChatMessageRole};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};
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
    working_messages: Vec<ChatMessage>,
    history_override: Option<Vec<ChatMessage>>,
    buffered_events: Vec<ThreadTurnEvent>,
    tool_events: Vec<ThreadToolEvent>,
}

impl ThreadCurrentTurn {
    fn new(external_message_id: Option<String>, started_at: DateTime<Utc>) -> Self {
        Self {
            turn_id: Uuid::new_v4(),
            external_message_id,
            started_at,
            working_messages: Vec::new(),
            history_override: None,
            buffered_events: Vec::new(),
            tool_events: Vec::new(),
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

    /// Export only the persisted non-system chat history currently owned by the thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.messages[self.system_prefix_len()..].to_vec()
    }

    /// Return the persisted leading system messages for the thread.
    pub fn system_prefix_messages(&self) -> &[ChatMessage] {
        &self.messages[..self.system_prefix_len()]
    }

    fn system_prefix_len(&self) -> usize {
        self.messages
            .iter()
            .take_while(|message| message.role == ChatMessageRole::System)
            .count()
    }
}

/// Unified persisted thread aggregate shared by session, command, and agent execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
        }
    }

    /// Rebind the runtime locator while keeping persisted thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    /// Load the flattened persisted non-system chat history for the thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.thread.load_messages()
    }

    /// Export the current request-visible messages owned by the thread.
    ///
    /// 当 turn 已经开始时，返回值会按顺序包含：
    /// 1. 稳定 system prefix
    /// 2. 已 finalized 的 non-system history，或 turn 内 compact 生成的 active history override
    /// 3. 当前 turn 的 working messages
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
    /// let _ = thread.ensure_system_prompt_snapshot("system", now);
    /// thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .push_turn_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should buffer");
    ///
    /// assert_eq!(thread.messages()[0].role, ChatMessageRole::System);
    /// assert_eq!(thread.messages().last().unwrap().content, "hello");
    /// ```
    pub fn messages(&self) -> Vec<ChatMessage> {
        let mut messages = self.system_prefix_messages().to_vec();
        messages.extend(
            self.current_turn
                .as_ref()
                .and_then(|turn| turn.history_override.clone())
                .unwrap_or_else(|| self.thread.load_messages()),
        );
        if let Some(current_turn) = &self.current_turn {
            messages.extend(current_turn.working_messages.clone());
        }
        messages
    }

    /// Replace all persisted non-system messages while preserving the leading system prefix.
    pub(crate) fn replace_non_system_messages(
        &mut self,
        replacement: Vec<ChatMessage>,
        updated_at: DateTime<Utc>,
    ) {
        let mut messages = self.system_prefix_messages().to_vec();
        messages.extend(replacement);
        self.thread.messages = messages;
        self.thread.updated_at = updated_at;
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return the persisted thread-scoped system prompt snapshot.
    pub fn system_prefix_messages(&self) -> &[ChatMessage] {
        self.thread.system_prefix_messages()
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

    /// Return the current turn working messages that can become finalized history.
    pub fn current_turn_working_messages(&self) -> Vec<ChatMessage> {
        self.current_turn
            .as_ref()
            .map(|turn| turn.working_messages.clone())
            .unwrap_or_default()
    }

    /// Return the current buffered turn events.
    pub fn current_turn_events(&self) -> Vec<ThreadTurnEvent> {
        self.current_turn
            .as_ref()
            .map(|turn| turn.buffered_events.clone())
            .unwrap_or_default()
    }

    /// Return the active non-system message view owned by the thread.
    ///
    /// 这个视图会被 compact 使用。它包含已 finalized 的 non-system history 与当前 turn 中
    /// 已经物化的 working messages。
    pub fn active_non_system_messages(&self) -> Vec<ChatMessage> {
        let mut messages = self
            .current_turn
            .as_ref()
            .and_then(|turn| turn.history_override.clone())
            .unwrap_or_else(|| self.thread.load_messages());
        if let Some(current_turn) = &self.current_turn {
            messages.extend(current_turn.working_messages.clone());
        }
        messages
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
    /// let _ = thread.ensure_system_prompt_snapshot("system", now);
    /// let turn_id = thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .push_turn_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should be buffered");
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

    /// Append one request-visible message into the current turn working set.
    pub fn push_turn_message(&mut self, message: ChatMessage) -> Result<()> {
        let current_turn = self.current_turn_mut()?;
        current_turn.working_messages.push(message);
        Ok(())
    }

    /// Buffer one user-visible turn event that will only be dispatched after finalization.
    pub fn buffer_turn_event(&mut self, event: ThreadTurnEvent) -> Result<()> {
        let current_turn = self.current_turn_mut()?;
        current_turn.buffered_events.push(event);
        Ok(())
    }

    /// Replace the active non-system view with compacted messages for the current turn.
    ///
    /// Compact 会直接改写 thread-owned active view；原本已 finalized 的 non-system history
    /// 与当前 turn working set 会一起收敛到新的 active history override。
    pub fn apply_turn_compaction(&mut self, compacted_messages: Vec<ChatMessage>) -> Result<()> {
        let current_turn = self.current_turn_mut()?;
        current_turn.history_override = Some(compacted_messages);
        current_turn.working_messages.clear();
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
    /// let _ = thread.ensure_system_prompt_snapshot("system", now);
    /// thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .push_turn_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should be buffered");
    /// thread
    ///     .push_turn_message(ChatMessage::new(ChatMessageRole::Assistant, "world", now))
    ///     .expect("assistant reply should be buffered");
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
    /// assert_eq!(finalized.snapshot.load_messages().len(), 2);
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
    /// 异常失败会丢弃当前 turn 内尚未持久化的 working messages、tool events 和 buffered
    /// events；最终 snapshot 会回退到本轮开始前的线程状态，只保留对外错误事件。
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
    /// thread.store_turn(
    ///     Some("msg_0".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::Assistant, "persisted", now)],
    ///     now,
    ///     now,
    /// );
    /// thread
    ///     .begin_turn(Some("msg_1".to_string()), now)
    ///     .expect("turn should start");
    /// thread
    ///     .push_turn_message(ChatMessage::new(ChatMessageRole::User, "hello", now))
    ///     .expect("user message should be buffered");
    ///
    /// let finalized = thread
    ///     .finalize_turn_failure("network error", now)
    ///     .expect("failed turn should finalize");
    ///
    /// assert!(matches!(
    ///     finalized.status,
    ///     ThreadFinalizedTurnStatus::Failed { .. }
    /// ));
    /// assert_eq!(finalized.snapshot.load_messages().len(), 1);
    /// assert_eq!(finalized.snapshot.load_messages()[0].content, "persisted");
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
            dropped_working_count = current_turn.working_messages.len(),
            dropped_event_count = current_turn.buffered_events.len(),
            dropped_tool_event_count = current_turn.tool_events.len(),
            error = %error,
            "thread-owned turn failed; dropping in-flight turn contents"
        );

        Ok(ThreadFinalizedTurn {
            turn_id: current_turn.turn_id,
            external_message_id: current_turn.external_message_id,
            started_at: current_turn.started_at,
            completed_at,
            reply: failure_reply.clone(),
            status: ThreadFinalizedTurnStatus::Failed { error },
            events: vec![ThreadTurnEvent {
                kind: ThreadTurnEventKind::TextOutput,
                content: failure_reply,
                metadata: json!({
                    "source": "turn_failure",
                    "is_final": true,
                    "is_error": true,
                }),
            }],
            snapshot: self.clone(),
        })
    }

    /// Initialize the thread-scoped system prompt snapshot on first use.
    pub fn ensure_system_prompt_snapshot(
        &mut self,
        system_prompt: impl AsRef<str>,
        created_at: DateTime<Utc>,
    ) -> bool {
        let system_prompt = system_prompt.as_ref().trim();
        if system_prompt.is_empty() || !self.thread.system_prefix_messages().is_empty() {
            return false;
        }

        self.initialize_system_messages(
            vec![ChatMessage::new(
                ChatMessageRole::System,
                system_prompt,
                created_at,
            )],
            "system_prompt",
        )
    }

    /// Initialize persisted system messages from one prebuilt snapshot when missing.
    pub fn ensure_system_prefix_messages(&mut self, system_messages: &[ChatMessage]) -> bool {
        if !self.thread.system_prefix_messages().is_empty() {
            return false;
        }

        let normalized_messages = system_messages
            .iter()
            .filter(|message| !message.content.trim().is_empty())
            .map(|message| {
                ChatMessage::new(
                    ChatMessageRole::System,
                    message.content.clone(),
                    message.created_at,
                )
            })
            .collect::<Vec<_>>();
        if normalized_messages.is_empty() {
            return false;
        }

        self.initialize_system_messages(normalized_messages, "thread_init")
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

    /// Store one completed turn through the thread-owned turn finalization path.
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
    /// let turn_id = thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// assert_eq!(thread.load_messages().len(), 1);
    /// assert_ne!(turn_id, uuid::Uuid::nil());
    /// ```
    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        let reply = messages
            .iter()
            .rev()
            .find(|message| message.role == ChatMessageRole::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        self.open_turn(external_message_id, started_at)
            .expect("store_turn should open one compatibility turn");
        let current_turn = self
            .current_turn
            .as_mut()
            .expect("compatibility turn should exist");
        current_turn.working_messages = messages;
        self.finalize_turn_success(reply, completed_at)
            .expect("store_turn should finalize one compatibility turn")
            .turn_id
    }

    /// Store one completed turn together with explicit runtime tool state compatibility payloads.
    pub fn store_turn_state(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> Uuid {
        self.replace_loaded_toolsets(loaded_toolsets);
        self.pending_tool_events.extend(tool_events);
        self.store_turn(external_message_id, messages, started_at, completed_at)
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

        let mut finalized_non_system_messages = current_turn
            .history_override
            .unwrap_or_else(|| self.thread.load_messages());
        finalized_non_system_messages.extend(current_turn.working_messages);
        self.replace_non_system_messages(finalized_non_system_messages, completed_at);
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

    fn initialize_system_messages(
        &mut self,
        system_messages: Vec<ChatMessage>,
        source: &str,
    ) -> bool {
        if system_messages.is_empty() || !self.thread.system_prefix_messages().is_empty() {
            return false;
        }

        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            source,
            system_message_count = system_messages.len(),
            "initialized persisted thread system messages"
        );
        self.thread.messages.splice(0..0, system_messages);
        true
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
