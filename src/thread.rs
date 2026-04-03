//! Conversation-thread persistence types used by the session manager.

use crate::{
    compact::ContextBudgetReport,
    context::{ChatMessage, ChatMessageRole},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};
use tracing::info;
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

fn resolve_internal_thread_id(thread_id: &str) -> Uuid {
    Uuid::parse_str(thread_id).unwrap_or_else(|_| derive_internal_thread_id(thread_id))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

impl ConversationTurn {
    /// Create a new stored turn from normalized chat messages.
    pub fn new(
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Self {
        Self::with_id(
            Uuid::new_v4(),
            external_message_id,
            messages,
            started_at,
            completed_at,
        )
    }

    fn with_id(
        id: Uuid,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            external_message_id,
            messages,
            started_at,
            completed_at,
        }
    }

    /// Return the final assistant-visible message recorded in this turn.
    pub fn final_assistant_message(&self) -> Option<&ChatMessage> {
        self.messages.iter().rev().find(|message| {
            message.role == ChatMessageRole::Assistant && !message.content.trim().is_empty()
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThreadToolEventKind {
    LoadToolset,
    UnloadToolset,
    ExecuteTool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Stable runtime locator shared by session, command, and agent execution on one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadContextLocator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>, // TODO: 不应该有None？
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

    /// Build a synthetic locator for deprecated thread-id-only compatibility paths.
    pub fn for_internal_thread(thread_id: impl Into<String>) -> Self {
        Self::new(None, "compat", "compat", "compat", thread_id)
    }
}

/// Persisted conversation and tool audit history for one internal thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadConversation {
    pub external_thread_id: String,
    pub turns: Vec<ConversationTurn>,
    #[serde(default)]
    pub tool_events: Vec<ThreadToolEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ThreadConversation {
    /// Create an empty thread conversation.
    pub fn new(external_thread_id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            external_thread_id: external_thread_id.into(),
            turns: Vec::new(),
            tool_events: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Load the turn for the incoming external message id or create it on first sight.
    pub fn load_or_create_turn(
        &mut self,
        external_message_id: Option<String>,
        turn_id: Uuid,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> &mut ConversationTurn {
        if let Some(message_id) = external_message_id.as_deref() {
            if let Some(index) = self
                .turns
                .iter()
                .position(|turn| turn.external_message_id.as_deref() == Some(message_id))
            {
                let turn = &mut self.turns[index];
                turn.started_at = started_at;
                turn.completed_at = completed_at;
                return turn;
            }
        }

        self.turns.push(ConversationTurn::with_id(
            turn_id,
            external_message_id,
            Vec::new(),
            started_at,
            completed_at,
        ));
        self.updated_at = completed_at;
        self.turns
            .last_mut()
            .expect("turn should exist immediately after insertion")
    }

    /// Store one normalized turn payload into the conversation, creating the turn on first sight.
    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        self.store_turn_events(
            external_message_id,
            messages,
            started_at,
            completed_at,
            Vec::new(),
        )
    }

    /// Store one normalized turn payload together with structured tool audit events.
    pub fn store_turn_events(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> Uuid {
        let turn_id = {
            let turn = self.load_or_create_turn(
                external_message_id,
                Uuid::new_v4(),
                started_at,
                completed_at,
            );
            turn.messages = messages;
            turn.started_at = started_at;
            turn.completed_at = completed_at;
            turn.id
        };
        self.tool_events.extend(
            tool_events
                .into_iter()
                .map(|event| event.with_turn_id(turn_id)),
        );
        self.updated_at = completed_at;
        turn_id
    }

    /// Replace the active history view while keeping the current thread identity.
    pub fn overwrite_active_history(&mut self, replacement: &ThreadConversation) {
        self.turns = replacement.turns.clone();
        self.tool_events = replacement.tool_events.clone();
        self.updated_at = replacement.updated_at;
    }

    /// Load the flattened message history for the whole thread conversation.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.turns
            .iter()
            .flat_map(|turn| turn.messages.iter().cloned())
            .collect()
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.tool_events.clone()
    }

    /// Project the thread conversation into the legacy `ConversationThread` compatibility shape.
    pub fn to_legacy_thread(
        &self,
        thread_id: Uuid,
        loaded_toolsets: &[String],
    ) -> ConversationThread {
        ConversationThread {
            id: thread_id,
            external_thread_id: self.external_thread_id.clone(),
            turns: self.turns.clone(),
            loaded_toolsets: normalize_loaded_toolsets(loaded_toolsets.to_vec()),
            tool_events: self.tool_events.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl From<ConversationThread> for ThreadConversation {
    fn from(value: ConversationThread) -> Self {
        Self {
            external_thread_id: value.external_thread_id,
            turns: value.turns,
            tool_events: value.tool_events,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&ConversationThread> for ThreadConversation {
    fn from(value: &ConversationThread) -> Self {
        Self {
            external_thread_id: value.external_thread_id.clone(),
            turns: value.turns.clone(),
            tool_events: value.tool_events.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// Thread-scoped compact visibility projection derived from the current request budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadCompactToolProjection {
    pub auto_compact: bool,
    pub visible: bool,
    pub budget_report: ContextBudgetReport,
}

/// Thread feature flags and runtime feature overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadFeatureState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_enabled_override: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_override: Option<bool>,
}

/// Thread-scoped tool runtime state owned by `ThreadContext`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadToolState {
    #[serde(default)]
    pub loaded_toolsets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_tool_projection: Option<ThreadCompactToolProjection>,
}

/// Thread-scoped request context snapshot that remains stable across turns.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadRequestContext {
    #[serde(default)]
    pub system: Vec<ChatMessage>,
}

/// Fixed feature system-prompt slots exported ahead of persisted conversation history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadFeaturesSystemPrompt {
    pub toolset_catalog: Vec<ChatMessage>,
    pub skill_catalog: Vec<ChatMessage>,
    pub auto_compact: Vec<ChatMessage>,
}

impl ThreadFeaturesSystemPrompt {
    /// Export the fixed feature system-prompt slots in stable request order.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::ThreadFeaturesSystemPrompt,
    /// };
    ///
    /// let mut slots = ThreadFeaturesSystemPrompt::default();
    /// slots.toolset_catalog.push(ChatMessage::new(
    ///     ChatMessageRole::System,
    ///     "toolset",
    ///     Utc::now(),
    /// ));
    /// slots.auto_compact.push(ChatMessage::new(
    ///     ChatMessageRole::System,
    ///     "auto-compact",
    ///     Utc::now(),
    /// ));
    ///
    /// assert_eq!(
    ///     slots
    ///         .ordered_messages()
    ///         .into_iter()
    ///         .map(|message| message.content)
    ///         .collect::<Vec<_>>(),
    ///     vec!["toolset".to_string(), "auto-compact".to_string()]
    /// );
    /// ```
    pub fn ordered_messages(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::with_capacity(self.message_count());
        messages.extend(self.toolset_catalog.iter().cloned());
        messages.extend(self.skill_catalog.iter().cloned());
        messages.extend(self.auto_compact.iter().cloned());
        messages
    }

    pub(crate) fn message_count(&self) -> usize {
        self.toolset_catalog.len() + self.skill_catalog.len() + self.auto_compact.len()
    }
}

#[derive(Debug, Clone, Default)]
struct ThreadLiveFeatureInputs {
    runtime_system: Vec<ChatMessage>,
    memory: Vec<ChatMessage>,
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

/// Full thread-scoped runtime state separated from persisted conversation history.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ThreadState {
    #[serde(default)]
    pub features: ThreadFeatureState,
    #[serde(default)]
    pub request_context: ThreadRequestContext,
    #[serde(default)]
    pub tools: ThreadToolState,
    #[serde(default)]
    pub approval: ThreadApprovalState,
}

/// Unified runtime host for one internal thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadContext {
    pub locator: ThreadContextLocator,
    pub conversation: ThreadConversation,
    #[serde(default)]
    pub state: ThreadState,
    #[serde(default, skip_serializing, skip_deserializing)]
    revision: u64,
    #[serde(default, skip_serializing, skip_deserializing)]
    live_feature_inputs: ThreadLiveFeatureInputs,
    #[serde(default, skip_serializing, skip_deserializing)]
    features_system_prompt: ThreadFeaturesSystemPrompt,
    #[serde(default, skip_serializing, skip_deserializing)]
    live_system_messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing, skip_deserializing)]
    live_memory_messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing, skip_deserializing)]
    live_chat_messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing, skip_deserializing)]
    pending_tool_events: Vec<ThreadToolEvent>,
}

impl ThreadContext {
    /// Create one empty thread context.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{ThreadContext, ThreadContextLocator};
    ///
    /// let context = ThreadContext::new(
    ///     ThreadContextLocator::new(
    ///         Some("session-1".to_string()),
    ///         "feishu",
    ///         "ou_xxx",
    ///         "thread_ext",
    ///         "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    ///     ),
    ///     Utc::now(),
    /// );
    /// assert_eq!(context.locator.thread_id, "3d71df7b-8708-5b55-a1a8-4627ae6e30c1");
    /// assert_eq!(context.locator.thread_key(), "ou_xxx:feishu:thread_ext");
    /// ```
    pub fn new(locator: ThreadContextLocator, now: DateTime<Utc>) -> Self {
        let external_thread_id = locator.external_thread_id.clone();
        Self {
            locator,
            conversation: ThreadConversation::new(external_thread_id, now),
            state: ThreadState::default(),
            revision: 0,
            live_feature_inputs: ThreadLiveFeatureInputs::default(),
            features_system_prompt: ThreadFeaturesSystemPrompt::default(),
            live_system_messages: Vec::new(),
            live_memory_messages: Vec::new(),
            live_chat_messages: Vec::new(),
            pending_tool_events: Vec::new(),
        }
    }

    /// Build a thread context from the legacy `ConversationThread` compatibility shape.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{ConversationThread, ThreadContext, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let legacy = ConversationThread::new("thread_ext", now);
    /// let context = ThreadContext::from_conversation_thread(
    ///     ThreadContextLocator::new(
    ///         None,
    ///         "feishu",
    ///         "ou_xxx",
    ///         "thread_ext",
    ///         "3d71df7b-8708-5b55-a1a8-4627ae6e30c1",
    ///     ),
    ///     legacy,
    /// );
    /// assert_eq!(context.locator.external_thread_id, "thread_ext");
    /// ```
    pub fn from_conversation_thread(
        locator: ThreadContextLocator,
        thread: ConversationThread,
    ) -> Self {
        let loaded_toolsets = thread.loaded_toolsets.clone();
        Self {
            locator,
            conversation: ThreadConversation::from(thread),
            state: ThreadState {
                features: ThreadFeatureState::default(),
                request_context: ThreadRequestContext::default(),
                tools: ThreadToolState {
                    loaded_toolsets: normalize_loaded_toolsets(loaded_toolsets),
                    compact_tool_projection: None,
                },
                approval: ThreadApprovalState::default(),
            },
            revision: 0,
            live_feature_inputs: ThreadLiveFeatureInputs::default(),
            features_system_prompt: ThreadFeaturesSystemPrompt::default(),
            live_system_messages: Vec::new(),
            live_memory_messages: Vec::new(),
            live_chat_messages: Vec::new(),
            pending_tool_events: Vec::new(),
        }
    }

    /// Rebind the runtime locator while keeping conversation and thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    /// Load the flattened message history for the whole thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.conversation.load_messages()
    }

    /// Export one LLM-facing message sequence from the current thread.
    ///
    /// This keeps persisted request context, fixed feature system prompt, transient runtime
    /// messages, and the current working chat assembled behind the thread boundary instead of
    /// rebuilding them in the agent loop.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// let _ = context.ensure_system_prompt_snapshot("system prompt", now);
    /// context.push_message(ChatMessage::new(ChatMessageRole::Memory, "transient", now));
    /// context.push_message(ChatMessage::new(ChatMessageRole::User, "hello", now));
    ///
    /// let exported = context.messages();
    ///
    /// assert_eq!(exported[0].content, "system prompt");
    /// assert_eq!(exported[1].content, "transient");
    /// assert_eq!(exported[2].content, "hello");
    /// ```
    pub fn messages(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::with_capacity(
            self.state.request_context.system.len()
                + self.features_system_prompt.message_count()
                + self.live_system_messages.len()
                + self.live_memory_messages.len()
                + self
                    .conversation
                    .turns
                    .iter()
                    .map(|turn| turn.messages.len())
                    .sum::<usize>()
                + self.live_chat_messages.len(),
        );
        messages.extend(self.state.request_context.system.iter().cloned());
        messages.extend(self.features_system_prompt.ordered_messages());
        messages.extend(self.live_system_messages.iter().cloned());
        messages.extend(self.live_memory_messages.iter().cloned());
        messages.extend(self.conversation.load_messages());
        messages.extend(self.live_chat_messages.iter().cloned());
        messages
    }

    /// Push one live chat message into the current thread working set.
    ///
    /// System/memory messages are treated as request-time transient inputs. User/assistant/tool
    /// messages are appended to the live chat area and can participate in compaction.
    pub fn push_message(&mut self, message: ChatMessage) {
        match message.role {
            ChatMessageRole::System => {
                self.live_feature_inputs
                    .runtime_system
                    .push(message.clone());
                self.live_system_messages.push(message);
            }
            ChatMessageRole::Memory => {
                self.live_feature_inputs.memory.push(message.clone());
                self.live_memory_messages.push(message);
            }
            _ => {
                self.live_chat_messages.push(message);
            }
        }
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return the persisted thread-scoped system prompt snapshot.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{ThreadContext, ThreadContextLocator};
    ///
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     Utc::now(),
    /// );
    /// context.ensure_system_prompt_snapshot("system prompt", Utc::now());
    ///
    /// assert_eq!(context.request_context_system_messages().len(), 1);
    /// ```
    pub fn request_context_system_messages(&self) -> &[ChatMessage] {
        &self.state.request_context.system
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.conversation.load_tool_events()
    }

    /// Return the current pending tool events that still need to be bound to one stored turn.
    pub fn pending_tool_events(&self) -> &[ThreadToolEvent] {
        &self.pending_tool_events
    }

    /// Return the request-time memory inputs that a memory feature provider can materialize.
    pub fn request_memory_messages(&self) -> &[ChatMessage] {
        &self.live_feature_inputs.memory
    }

    /// Return the current fixed feature system-prompt slots.
    pub fn features_system_prompt(&self) -> &ThreadFeaturesSystemPrompt {
        &self.features_system_prompt
    }

    /// Record one thread-scoped tool event on the current runtime context.
    pub fn record_tool_event(&mut self, event: ThreadToolEvent) {
        self.pending_tool_events.push(event);
    }

    /// Initialize the thread-scoped system prompt snapshot on first use.
    ///
    /// Existing threads keep the first persisted snapshot; later calls are ignored so restored
    /// threads continue using the same stable prefix.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::{ThreadContext, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    ///
    /// assert!(context.ensure_system_prompt_snapshot("system prompt", now));
    /// assert!(!context.ensure_system_prompt_snapshot("new prompt", now));
    /// assert_eq!(
    ///     context.request_context_system_messages()[0].content,
    ///     "system prompt"
    /// );
    /// ```
    pub fn ensure_system_prompt_snapshot(
        &mut self,
        system_prompt: impl AsRef<str>,
        created_at: DateTime<Utc>,
    ) -> bool {
        let system_prompt = system_prompt.as_ref().trim();
        if system_prompt.is_empty() || !self.state.request_context.system.is_empty() {
            return false;
        }

        self.initialize_request_context_system_messages(
            vec![ChatMessage::new(
                ChatMessageRole::System,
                system_prompt,
                created_at,
            )],
            "system_prompt",
        )
    }

    /// Backfill one legacy system snapshot into the thread-scoped request context when missing.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// let system_messages = vec![ChatMessage::new(ChatMessageRole::System, "system", now)];
    ///
    /// assert!(context.ensure_request_context_system_messages(&system_messages));
    /// assert_eq!(
    ///     context.request_context_system_messages()[0].content,
    ///     "system"
    /// );
    /// ```
    pub fn ensure_request_context_system_messages(
        &mut self,
        system_messages: &[ChatMessage],
    ) -> bool {
        if !self.state.request_context.system.is_empty() {
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

        self.initialize_request_context_system_messages(normalized_messages, "legacy_context")
    }

    /// Replace the thread's loaded toolset state with a normalized snapshot.
    pub fn replace_loaded_toolsets(&mut self, loaded_toolsets: Vec<String>) {
        self.state.tools.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
    }

    /// Replace the request-time transient memory inputs for the current turn.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ThreadContext, ThreadContextLocator},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// context.replace_request_memory_messages(vec![ChatMessage::new(
    ///     ChatMessageRole::Memory,
    ///     "remember this",
    ///     now,
    /// )]);
    ///
    /// assert_eq!(context.request_memory_messages()[0].content, "remember this");
    /// ```
    pub fn replace_request_memory_messages(&mut self, memory_messages: Vec<ChatMessage>) {
        let normalized_messages = memory_messages
            .into_iter()
            .filter(|message| !message.content.trim().is_empty())
            .map(|message| {
                ChatMessage::new(ChatMessageRole::Memory, message.content, message.created_at)
            })
            .collect::<Vec<_>>();
        self.live_feature_inputs.memory = normalized_messages.clone();
        self.live_memory_messages = normalized_messages;
    }

    /// Mark one toolset as loaded for the current thread context.
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

    /// Mark one toolset as unloaded for the current thread context.
    pub fn unload_toolset(&mut self, toolset_name: &str) -> bool {
        let original_len = self.state.tools.loaded_toolsets.len();
        self.state
            .tools
            .loaded_toolsets
            .retain(|candidate| candidate != toolset_name);
        original_len != self.state.tools.loaded_toolsets.len()
    }

    /// Store one completed turn and bind all currently pending tool events to it.
    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        let tool_events = std::mem::take(&mut self.pending_tool_events);
        let turn_id = self.conversation.store_turn_events(
            external_message_id,
            messages,
            started_at,
            completed_at,
            tool_events,
        );
        self.state.tools.compact_tool_projection = None;
        turn_id
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

    /// Replace the active history view while keeping the current locator and state.
    pub fn overwrite_active_history(&mut self, replacement: &ThreadContext) {
        self.conversation
            .overwrite_active_history(&replacement.conversation);
        self.state = replacement.state.clone();
        self.live_feature_inputs = replacement.live_feature_inputs.clone();
        self.clear_live_turn_messages();
        self.pending_tool_events = replacement.pending_tool_events.clone();
    }

    /// Replace the active history view from one legacy `ConversationThread` snapshot.
    pub fn overwrite_active_history_from_conversation_thread(
        &mut self,
        replacement: &ConversationThread,
    ) {
        self.conversation = ThreadConversation::from(replacement);
        self.replace_loaded_toolsets(replacement.loaded_toolsets.clone());
        self.clear_live_turn_messages();
    }

    /// Clear the current thread back to one empty initial state.
    ///
    /// This drops all stored chat turns, tool events, request-context snapshots, loaded toolsets,
    /// feature overrides, approval state, and pending runtime tool events while keeping the
    /// current thread identity.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::{ThreadContext, ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// context.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// context.clear_to_initial_state(now);
    ///
    /// assert!(context.load_messages().is_empty());
    /// assert!(context.load_toolsets().is_empty());
    /// ```
    pub fn clear_to_initial_state(&mut self, now: DateTime<Utc>) {
        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            "clearing thread context back to initial state"
        );
        self.conversation = ThreadConversation::new(self.locator.external_thread_id.clone(), now);
        self.state = ThreadState::default();
        self.live_feature_inputs = ThreadLiveFeatureInputs::default();
        self.features_system_prompt = ThreadFeaturesSystemPrompt::default();
        self.live_system_messages.clear();
        self.live_memory_messages.clear();
        self.live_chat_messages.clear();
        self.pending_tool_events.clear();
    }

    /// Return the effective compact-enabled state for this thread context.
    pub fn compact_enabled(&self, default_enabled: bool) -> bool {
        self.state
            .features
            .compact_enabled_override
            .unwrap_or(default_enabled)
    }

    /// Return the effective auto-compact state for this thread context.
    pub fn auto_compact_enabled(&self, default_enabled: bool) -> bool {
        self.state
            .features
            .auto_compact_override
            .unwrap_or(default_enabled)
    }

    /// Update the compact-enabled override for the current thread context.
    pub fn set_compact_enabled_override(&mut self, enabled: Option<bool>) {
        self.state.features.compact_enabled_override = enabled;
    }

    /// Update the auto-compact override for the current thread context.
    pub fn set_auto_compact_override(&mut self, enabled: Option<bool>) {
        self.state.features.auto_compact_override = enabled;
    }

    /// Enable thread-scoped auto-compact on top of runtime compact for the current thread.
    pub fn enable_auto_compact(&mut self) {
        self.set_compact_enabled_override(Some(true));
        self.set_auto_compact_override(Some(true));
    }

    /// Disable thread-scoped auto-compact and fall back to the static compact default.
    pub fn disable_auto_compact(&mut self) {
        self.set_compact_enabled_override(None);
        self.set_auto_compact_override(Some(false));
        self.state.tools.compact_tool_projection = None;
    }

    /// Replace the current compact-tool visibility projection.
    pub fn set_compact_tool_projection(&mut self, projection: Option<ThreadCompactToolProjection>) {
        self.state.tools.compact_tool_projection = projection;
    }

    /// Return the current compact-tool visibility projection when present.
    pub fn compact_tool_projection(&self) -> Option<&ThreadCompactToolProjection> {
        self.state.tools.compact_tool_projection.as_ref()
    }

    /// Project the thread context back into the legacy `ConversationThread` compatibility shape.
    pub fn to_conversation_thread(&self) -> ConversationThread {
        self.conversation.to_legacy_thread(
            resolve_internal_thread_id(&self.locator.thread_id),
            &self.state.tools.loaded_toolsets,
        )
    }

    /// Replace the current fixed feature system-prompt slots with one rebuilt snapshot.
    ///
    /// Existing persisted snapshot/history stay untouched; only request-time feature slots change.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::{ThreadContext, ThreadContextLocator, ThreadFeaturesSystemPrompt},
    /// };
    ///
    /// let now = Utc::now();
    /// let mut context = ThreadContext::new(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "thread_ext", "thread_internal"),
    ///     now,
    /// );
    /// let mut slots = ThreadFeaturesSystemPrompt::default();
    /// slots.toolset_catalog.push(ChatMessage::new(
    ///     ChatMessageRole::System,
    ///     "toolset catalog",
    ///     now,
    /// ));
    /// context.rebuild_features_system_prompt(slots);
    ///
    /// assert_eq!(context.features_system_prompt().toolset_catalog.len(), 1);
    /// ```
    pub fn rebuild_features_system_prompt(
        &mut self,
        features_system_prompt: ThreadFeaturesSystemPrompt,
    ) {
        info!(
            thread_id = %self.locator.thread_id,
            toolset_catalog_count = features_system_prompt.toolset_catalog.len(),
            skill_catalog_count = features_system_prompt.skill_catalog.len(),
            auto_compact_count = features_system_prompt.auto_compact.len(),
            "rebuilt thread features system prompt"
        );
        self.features_system_prompt = features_system_prompt;
    }

    /// Replace the current transient runtime system messages while preserving request-time inputs.
    pub(crate) fn replace_live_system_messages(&mut self, system_messages: Vec<ChatMessage>) {
        let mut normalized_messages = self.live_feature_inputs.runtime_system.clone();
        normalized_messages.extend(system_messages.into_iter().filter(|message| {
            message.role == ChatMessageRole::System && !message.content.trim().is_empty()
        }));
        self.live_system_messages = normalized_messages;
    }

    /// Replace the current transient live memory messages with one rebuilt snapshot.
    pub(crate) fn rebuild_live_memory_messages(&mut self, memory_messages: Vec<ChatMessage>) {
        self.live_memory_messages = memory_messages;
    }

    pub(crate) fn clear_live_messages(&mut self) {
        self.live_feature_inputs = ThreadLiveFeatureInputs::default();
        self.features_system_prompt = ThreadFeaturesSystemPrompt::default();
        self.live_system_messages.clear();
        self.live_memory_messages.clear();
        self.live_chat_messages.clear();
    }

    pub(crate) fn clear_live_turn_messages(&mut self) {
        self.features_system_prompt = ThreadFeaturesSystemPrompt::default();
        self.live_system_messages.clear();
        self.live_memory_messages.clear();
        self.live_chat_messages.clear();
    }

    pub(crate) fn pending_chat_messages(&self) -> &[ChatMessage] {
        &self.live_chat_messages
    }

    fn initialize_request_context_system_messages(
        &mut self,
        system_messages: Vec<ChatMessage>,
        source: &str,
    ) -> bool {
        if system_messages.is_empty() || !self.state.request_context.system.is_empty() {
            return false;
        }

        info!(
            thread_id = %self.locator.thread_id,
            external_thread_id = %self.locator.external_thread_id,
            source,
            system_message_count = system_messages.len(),
            "initialized thread request context snapshot"
        );
        self.state.request_context.system = system_messages;
        true
    }
}

impl Deref for ThreadContext {
    type Target = ThreadConversation;

    fn deref(&self) -> &Self::Target {
        &self.conversation
    }
}

impl DerefMut for ThreadContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conversation
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationThread {
    pub id: Uuid,
    pub external_thread_id: String,
    pub turns: Vec<ConversationTurn>,
    #[serde(default)]
    pub loaded_toolsets: Vec<String>,
    #[serde(default)]
    pub tool_events: Vec<ThreadToolEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationThread {
    /// Create a detached legacy thread snapshot with a generated standalone id.
    ///
    /// This helper only exists for compatibility tests and detached in-memory snapshots.
    /// Runtime thread identity should come from `ThreadContextLocator.thread_id`.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let thread = ConversationThread::new("default", Utc::now());
    /// assert_eq!(thread.external_thread_id, "default");
    /// ```
    pub fn new(external_thread_id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self::with_id(Uuid::new_v4(), external_thread_id, now)
    }

    /// Create a detached legacy thread snapshot with an explicit internal thread id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    /// use uuid::Uuid;
    ///
    /// let thread_id = Uuid::new_v4();
    /// let thread = ConversationThread::with_id(thread_id, "default", Utc::now());
    /// assert_eq!(thread.id, thread_id);
    /// ```
    pub fn with_id(id: Uuid, external_thread_id: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            id,
            external_thread_id: external_thread_id.into(),
            turns: Vec::new(),
            loaded_toolsets: Vec::new(),
            tool_events: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Load the turn for the incoming external message id or create it on first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::thread::ConversationThread;
    /// use uuid::Uuid;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// let turn_id = Uuid::new_v4();
    ///
    /// let first_id = thread
    ///     .load_or_create_turn(Some("msg_1".to_string()), turn_id, now, now)
    ///     .id;
    /// let second_id = thread
    ///     .load_or_create_turn(Some("msg_1".to_string()), Uuid::new_v4(), now, now)
    ///     .id;
    ///
    /// assert_eq!(first_id, second_id);
    /// ```
    pub fn load_or_create_turn(
        &mut self,
        external_message_id: Option<String>,
        turn_id: Uuid,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> &mut ConversationTurn {
        if let Some(message_id) = external_message_id.as_deref() {
            if let Some(index) = self
                .turns
                .iter()
                .position(|turn| turn.external_message_id.as_deref() == Some(message_id))
            {
                let turn = &mut self.turns[index];
                turn.started_at = started_at;
                turn.completed_at = completed_at;
                return turn;
            }
        }

        self.turns.push(ConversationTurn::with_id(
            turn_id,
            external_message_id,
            Vec::new(),
            started_at,
            completed_at,
        ));
        self.updated_at = completed_at;
        self.turns
            .last_mut()
            .expect("turn should exist immediately after insertion")
    }

    /// Store one normalized turn payload into the thread, creating the turn on first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// let turn_id = thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// assert_eq!(thread.turns[0].id, turn_id);
    /// ```
    pub fn store_turn(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> Uuid {
        self.store_turn_state(
            external_message_id,
            messages,
            started_at,
            completed_at,
            Vec::new(),
            Vec::new(),
        )
    }

    /// Store one normalized turn payload together with persisted tool runtime state.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// let turn_id = thread.store_turn_state(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    ///     vec!["browser".to_string()],
    ///     Vec::new(),
    /// );
    ///
    /// assert_eq!(thread.turns[0].id, turn_id);
    /// assert_eq!(thread.loaded_toolsets, vec!["browser".to_string()]);
    /// ```
    pub fn store_turn_state(
        &mut self,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> Uuid {
        let turn_id = {
            let turn = self.load_or_create_turn(
                external_message_id,
                Uuid::new_v4(),
                started_at,
                completed_at,
            );
            turn.messages = messages;
            turn.started_at = started_at;
            turn.completed_at = completed_at;
            turn.id
        };
        self.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
        self.tool_events.extend(
            tool_events
                .into_iter()
                .map(|event| event.with_turn_id(turn_id)),
        );
        self.updated_at = completed_at;
        turn_id
    }

    /// Replace the active history view while keeping the current thread identity.
    ///
    /// This is used by compact to swap old active chat turns with a compacted turn before the
    /// next completed turn is appended.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     context::{ChatMessage, ChatMessageRole},
    ///     thread::ConversationThread,
    /// };
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// let mut compacted = thread.clone();
    /// compacted.store_turn(
    ///     None,
    ///     vec![ChatMessage::new(ChatMessageRole::Assistant, "这是压缩后的上下文", now)],
    ///     now,
    ///     now,
    /// );
    ///
    /// thread.overwrite_active_history(&compacted);
    /// assert_eq!(thread.id, compacted.id);
    /// assert_eq!(thread.turns.len(), compacted.turns.len());
    /// ```
    pub fn overwrite_active_history(&mut self, replacement: &ConversationThread) {
        self.turns = replacement.turns.clone();
        self.loaded_toolsets = normalize_loaded_toolsets(replacement.loaded_toolsets.clone());
        self.tool_events = replacement.tool_events.clone();
        self.updated_at = replacement.updated_at;
    }

    /// Load the flattened message history for the whole thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.turns
            .iter()
            .flat_map(|turn| turn.messages.iter().cloned())
            .collect()
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.loaded_toolsets.clone()
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.tool_events.clone()
    }

    /// Project this legacy thread snapshot into one `ThreadContext` with separated runtime state.
    pub fn into_thread_context(self, locator: ThreadContextLocator) -> ThreadContext {
        ThreadContext::from_conversation_thread(locator, self)
    }
}

fn normalize_loaded_toolsets(mut loaded_toolsets: Vec<String>) -> Vec<String> {
    loaded_toolsets.retain(|name| !name.trim().is_empty());
    loaded_toolsets.sort();
    loaded_toolsets.dedup();
    loaded_toolsets
}

fn default_tool_event_metadata() -> Value {
    json!({})
}
