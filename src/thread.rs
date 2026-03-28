//! Conversation-thread persistence types used by the session manager.

use crate::{
    compact::ContextBudgetReport,
    context::{ChatMessage, ChatMessageRole},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};
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

    /// Retain only the latest `max_messages` across the whole thread conversation.
    pub fn retain_latest_messages(&mut self, max_messages: usize) {
        if max_messages == 0 {
            self.turns.clear();
            return;
        }

        let mut remaining_drop = self
            .turns
            .iter()
            .map(|turn| turn.messages.len())
            .sum::<usize>()
            .saturating_sub(max_messages);

        if remaining_drop == 0 {
            return;
        }

        for turn in &mut self.turns {
            if remaining_drop == 0 {
                break;
            }

            let turn_drop = remaining_drop.min(turn.messages.len());
            if turn_drop > 0 {
                turn.messages.drain(0..turn_drop);
                remaining_drop -= turn_drop;
            }
        }

        self.turns.retain(|turn| !turn.messages.is_empty());
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
                tools: ThreadToolState {
                    loaded_toolsets: normalize_loaded_toolsets(loaded_toolsets),
                    compact_tool_projection: None,
                },
                approval: ThreadApprovalState::default(),
            },
            pending_tool_events: Vec::new(),
        }
    }

    /// Rebind the runtime locator while keeping conversation and thread state intact.
    pub fn rebind_locator(&mut self, locator: ThreadContextLocator) {
        self.locator = locator;
    }

    /// Load the flattened message history for the whole thread.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        self.conversation.load_messages()
    }

    /// Return the persisted loaded toolsets for the thread.
    pub fn load_toolsets(&self) -> Vec<String> {
        self.state.tools.loaded_toolsets.clone()
    }

    /// Return the persisted structured tool event history.
    pub fn load_tool_events(&self) -> Vec<ThreadToolEvent> {
        self.conversation.load_tool_events()
    }

    /// Return the current pending tool events that still need to be bound to one stored turn.
    pub fn pending_tool_events(&self) -> &[ThreadToolEvent] {
        &self.pending_tool_events
    }

    /// Record one thread-scoped tool event on the current runtime context.
    pub fn record_tool_event(&mut self, event: ThreadToolEvent) {
        self.pending_tool_events.push(event);
    }

    /// Replace the thread's loaded toolset state with a normalized snapshot.
    pub fn replace_loaded_toolsets(&mut self, loaded_toolsets: Vec<String>) {
        self.state.tools.loaded_toolsets = normalize_loaded_toolsets(loaded_toolsets);
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
        self.pending_tool_events = replacement.pending_tool_events.clone();
    }

    /// Replace the active history view from one legacy `ConversationThread` snapshot.
    pub fn overwrite_active_history_from_conversation_thread(
        &mut self,
        replacement: &ConversationThread,
    ) {
        self.conversation = ThreadConversation::from(replacement);
        self.replace_loaded_toolsets(replacement.loaded_toolsets.clone());
    }

    /// Retain only the latest `max_messages` across the whole thread conversation.
    pub fn retain_latest_messages(&mut self, max_messages: usize) {
        self.conversation.retain_latest_messages(max_messages);
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

    /// Retain only the latest `max_messages` across the whole thread.
    ///
    /// Empty turns left behind by trimming are removed so the stored thread shape converges with
    /// the retained history window.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::thread::ConversationThread;
    ///
    /// let now = Utc::now();
    /// let mut thread = ConversationThread::new("default", now);
    /// thread.store_turn(
    ///     Some("msg_1".to_string()),
    ///     vec![
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_0", now),
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_1", now),
    ///         ChatMessage::new(ChatMessageRole::Assistant, "message_2", now),
    ///     ],
    ///     now,
    ///     now,
    /// );
    ///
    /// thread.retain_latest_messages(2);
    ///
    /// assert_eq!(
    ///     thread
    ///         .load_messages()
    ///         .into_iter()
    ///         .map(|message| message.content)
    ///         .collect::<Vec<_>>(),
    ///     vec!["message_1".to_string(), "message_2".to_string()]
    /// );
    /// ```
    pub fn retain_latest_messages(&mut self, max_messages: usize) {
        if max_messages == 0 {
            self.turns.clear();
            return;
        }

        let mut remaining_drop = self
            .turns
            .iter()
            .map(|turn| turn.messages.len())
            .sum::<usize>()
            .saturating_sub(max_messages);

        if remaining_drop == 0 {
            return;
        }

        for turn in &mut self.turns {
            if remaining_drop == 0 {
                break;
            }

            let turn_drop = remaining_drop.min(turn.messages.len());
            if turn_drop > 0 {
                turn.messages.drain(0..turn_drop);
                remaining_drop -= turn_drop;
            }
        }

        self.turns.retain(|turn| !turn.messages.is_empty());
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
