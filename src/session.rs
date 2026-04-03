//! Session cache and persistent thread-context storage orchestration.
//!
//! `SessionManager` owns the hot in-process cache for active sessions and delegates durable
//! persistence to a pluggable `SessionStore` backend. The cache keeps one thread-level mutex for
//! each live `ThreadContext`, while the store still persists detached snapshots so thread
//! recovery, tool state restoration, and external-message deduplication stay consistent across
//! restarts.

pub mod store;

use crate::context::ChatMessage;
use crate::model::IncomingMessage;
use crate::thread::{
    ConversationThread, ThreadContext, ThreadContextLocator, ThreadToolEvent,
    derive_internal_thread_id,
};
use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::info;
use uuid::Uuid;

pub use store::{
    ExternalMessageDedupRecord, MemorySessionStore, SessionRevisionConflict, SessionStore,
    SessionStoreError, SessionStoreResult, SqliteSessionStore, StoredSessionRecord,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionKey {
    pub channel: String,
    pub user_id: String,
}

impl SessionKey {
    /// Build a stable session key from a normalized incoming message.
    pub fn from_incoming(incoming: &IncomingMessage) -> Self {
        Self {
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
        }
    }

    /// Return the normalized thread key for one external thread inside this session.
    ///
    /// `thread_key` follows the contract `user:channel:external_thread_id`.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionKey;
    ///
    /// let key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    ///
    /// assert_eq!(key.thread_key("thread_ext"), "ou_xxx:feishu:thread_ext");
    /// ```
    pub fn thread_key(&self, external_thread_id: &str) -> String {
        format!("{}:{}:{}", self.user_id, self.channel, external_thread_id)
    }

    /// Derive the stable internal thread id for one external thread inside this session.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionKey;
    ///
    /// let key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    ///
    /// let thread_id = key.derive_thread_id("thread_ext");
    /// assert_eq!(thread_id, key.derive_thread_id("thread_ext"));
    /// ```
    pub fn derive_thread_id(&self, external_thread_id: &str) -> Uuid {
        derive_internal_thread_id(&self.thread_key(external_thread_id))
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ThreadLocator {
    pub session_id: Uuid,
    pub channel: String,
    pub user_id: String,
    pub external_thread_id: String,
    pub thread_id: Uuid,
}

impl ThreadLocator {
    /// Build a resolved session-thread locator from one incoming message and the derived ids.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::{SessionKey, ThreadLocator};
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: None,
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let session_id = Uuid::new_v4();
    /// let thread_id = SessionKey::from_incoming(&incoming).derive_thread_id("default");
    /// let locator = ThreadLocator::new(session_id, &incoming, "default", thread_id);
    /// assert_eq!(locator.external_thread_id, "default");
    /// assert_eq!(locator.thread_id, thread_id);
    /// assert_eq!(locator.thread_key(), "ou_xxx:feishu:default");
    /// ```
    pub fn new(
        session_id: Uuid,
        incoming: &IncomingMessage,
        external_thread_id: impl Into<String>,
        thread_id: Uuid,
    ) -> Self {
        Self {
            session_id,
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
            external_thread_id: external_thread_id.into(),
            thread_id,
        }
    }

    /// Return the parent session key for this thread locator.
    pub fn session_key(&self) -> SessionKey {
        SessionKey {
            channel: self.channel.clone(),
            user_id: self.user_id.clone(),
        }
    }

    /// Return the normalized thread key for this resolved thread locator.
    pub fn thread_key(&self) -> String {
        self.session_key().thread_key(&self.external_thread_id)
    }
}

impl From<&ThreadLocator> for ThreadContextLocator {
    fn from(value: &ThreadLocator) -> Self {
        Self::new(
            Some(value.session_id.to_string()),
            value.channel.clone(),
            value.user_id.clone(),
            value.external_thread_id.clone(),
            value.thread_id.to_string(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub key: SessionKey,
    pub threads: HashMap<Uuid, ThreadContext>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create an empty session snapshot.
    pub fn new(key: SessionKey, now: DateTime<Utc>) -> Self {
        Self::with_id(Uuid::new_v4(), key, now)
    }

    fn with_id(id: Uuid, key: SessionKey, now: DateTime<Utc>) -> Self {
        Self {
            id,
            key,
            threads: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Resolve one normalized external thread id into an internal thread context.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{session::{Session, SessionKey}, thread::ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let session_key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    /// let mut session = Session::new(session_key.clone(), now);
    /// let thread_id = session_key.derive_thread_id("default");
    /// let thread = session.load_or_create_thread(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
    ///     now,
    /// );
    ///
    /// assert_eq!(thread.locator.thread_id, thread_id.to_string());
    /// assert_eq!(thread.locator.external_thread_id, "default");
    /// assert_eq!(thread.locator.thread_key(), "ou_xxx:feishu:default");
    /// ```
    pub fn load_or_create_thread(
        &mut self,
        locator: ThreadContextLocator,
        now: DateTime<Utc>,
    ) -> &mut ThreadContext {
        let thread_id = Uuid::parse_str(&locator.thread_id)
            .expect("thread context locator should carry a UUID thread_id");
        self.threads
            .entry(thread_id)
            .or_insert_with(|| ThreadContext::new(locator, now))
    }

    /// Return the internal thread id currently bound to one normalized external thread id.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{session::{Session, SessionKey}, thread::ThreadContextLocator};
    ///
    /// let now = Utc::now();
    /// let session_key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    /// let mut session = Session::new(session_key.clone(), now);
    /// let thread_id = session_key.derive_thread_id("default");
    /// session.load_or_create_thread(
    ///     ThreadContextLocator::new(None, "feishu", "ou_xxx", "default", thread_id.to_string()),
    ///     now,
    /// );
    ///
    /// assert_eq!(session.thread_id_for_external("default"), Some(thread_id));
    /// ```
    pub fn thread_id_for_external(&self, external_thread_id: &str) -> Option<Uuid> {
        let thread_id = self.key.derive_thread_id(external_thread_id);
        self.threads.contains_key(&thread_id).then_some(thread_id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct StoredThreadState {
    pub thread_context: Option<ThreadContext>,
    pub thread: Option<ConversationThread>,
    pub messages: Vec<ChatMessage>,
    pub loaded_toolsets: Vec<String>,
    pub tool_events: Vec<ThreadToolEvent>,
}

type SharedThreadContext = Arc<Mutex<ThreadContext>>;

#[derive(Debug, Clone)]
struct CachedSession {
    id: Uuid,
    key: SessionKey,
    threads: HashMap<Uuid, SharedThreadContext>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl CachedSession {
    fn from_record(record: StoredSessionRecord) -> Self {
        Self {
            id: record.id,
            key: record.key,
            threads: HashMap::new(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

pub struct SessionManager {
    sessions: Mutex<HashMap<SessionKey, CachedSession>>,
    store: Arc<dyn SessionStore>,
}

impl SessionManager {
    /// Create an empty in-memory session manager.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionManager;
    ///
    /// let manager = SessionManager::new();
    /// let _ = manager;
    /// ```
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            store: Arc::new(MemorySessionStore::new()),
        }
    }

    /// Create a session manager backed by the provided persistent store.
    pub async fn with_store(store: Arc<dyn SessionStore>) -> SessionStoreResult<Self> {
        store.initialize_schema().await?;
        Ok(Self {
            sessions: Mutex::new(HashMap::new()),
            store,
        })
    }

    /// Resolve the external thread id on one incoming message and create the session/thread on
    /// first sight.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::SessionManager;
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_1".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let _future = manager.load_or_create_thread(&incoming);
    /// ```
    pub async fn load_or_create_thread(
        &self,
        incoming: &IncomingMessage,
    ) -> SessionStoreResult<ThreadLocator> {
        let session_key = SessionKey::from_incoming(incoming);
        let external_thread_id = incoming.resolved_external_thread_id();
        let thread_key = session_key.thread_key(&external_thread_id);
        let thread_id = session_key.derive_thread_id(&external_thread_id);
        let now = incoming.received_at;
        let session_record = self
            .store
            .resolve_or_create_session(&session_key, now)
            .await?;
        let locator =
            ThreadLocator::new(session_record.id, incoming, external_thread_id, thread_id);

        info!(
            session_id = %locator.session_id,
            channel = %incoming.channel,
            user_id = %incoming.user_id,
            external_thread_id = %locator.external_thread_id,
            thread_key = %thread_key,
            thread_id = %locator.thread_id,
            "resolved incoming thread identity"
        );

        let _ = self
            .ensure_thread_handle(&locator, Some(session_record.clone()), now)
            .await?;
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession::from_record(session_record.clone()));
        session.updated_at = now;

        Ok(locator)
    }

    /// Load the current flattened history for one channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     session_id: Uuid::new_v4(),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     external_thread_id: "default".to_string(),
    ///     thread_id: Uuid::new_v4(),
    /// };
    ///
    /// let _future = manager.load_turn(&locator);
    /// ```
    pub async fn load_turn(&self, locator: &ThreadLocator) -> SessionStoreResult<Vec<ChatMessage>> {
        Ok(self.load_thread_state(locator).await?.messages)
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<ThreadContext>> {
        if let Some(handle) = self.cached_thread_handle(locator).await {
            return Ok(Some(handle.lock().await.clone()));
        }

        let Some((session_record, stored_thread)) =
            self.fetch_thread_context_from_store(locator).await?
        else {
            return Ok(None);
        };
        let cached = self
            .cache_thread_handle_if_absent(
                &locator.session_key(),
                session_record,
                locator.thread_id,
                Arc::new(Mutex::new(stored_thread)),
                Utc::now(),
            )
            .await;
        Ok(Some(cached.lock().await.clone()))
    }

    /// Lock one live thread context for in-process mutation.
    ///
    /// Cache miss 会先从 store 恢复；store miss 会创建一个新的空线程上下文。
    /// 调用方拿到 guard 后可以直接修改线程；如果需要持久化，应在释放 guard 后调用
    /// `persist_thread_context(...)`，或者直接使用 `mutate_thread_context(...)`。
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::{
    ///     model::{IncomingMessage, ReplyTarget},
    ///     session::SessionManager,
    /// };
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// # async fn demo() -> openjarvis::session::SessionStoreResult<()> {
    /// let manager = SessionManager::new();
    /// let incoming = IncomingMessage {
    ///     id: Uuid::new_v4(),
    ///     external_message_id: Some("msg_1".to_string()),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     user_name: None,
    ///     content: "hello".to_string(),
    ///     external_thread_id: None,
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    ///
    /// let locator = manager.load_or_create_thread(&incoming).await?;
    /// let mut thread_context = manager.lock_thread_context(&locator, incoming.received_at).await?;
    /// thread_context.enable_auto_compact();
    ///
    /// assert!(thread_context.auto_compact_enabled(false));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn lock_thread_context(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<OwnedMutexGuard<ThreadContext>> {
        let handle = self.ensure_thread_handle(locator, None, now).await?;
        Ok(handle.lock_owned().await)
    }

    /// Persist the current cached thread context without appending a new turn.
    ///
    /// 这个入口适合配合 `lock_thread_context(...)` 使用: 调用方先拿到 guard 修改内存态，
    /// 释放 guard 后再调用这里把当前缓存里的最新线程快照写回 store。
    pub async fn persist_thread_context(
        &self,
        locator: &ThreadLocator,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let handle = self.ensure_thread_handle(locator, None, updated_at).await?;
        let snapshot = handle.lock().await.clone();
        let persisted = self
            .persist_thread_context_snapshot(locator, snapshot, updated_at)
            .await?;
        self.sync_thread_context_to_cache(&locator.session_key(), locator, &persisted, updated_at)
            .await;
        Ok(())
    }

    /// Mutate and persist one thread context in a single manager-owned critical section.
    ///
    /// 这个入口是对外推荐的修改方式: 它持有目标 thread 的锁，执行调用方的变更逻辑，
    /// 然后把同一份线程快照写回 store 并更新 revision，避免“改完忘记持久化”。
    pub async fn mutate_thread_context<R, F>(
        &self,
        locator: &ThreadLocator,
        updated_at: DateTime<Utc>,
        mutate: F,
    ) -> SessionStoreResult<R>
    where
        F: FnOnce(&mut ThreadContext) -> SessionStoreResult<R>,
    {
        let mut thread_context = self.lock_thread_context(locator, updated_at).await?;
        let result = mutate(&mut thread_context)?;
        let persisted = self
            .persist_thread_context_snapshot(locator, thread_context.clone(), updated_at)
            .await?;
        *thread_context = persisted;
        info!(
            thread_id = %locator.thread_id,
            updated_at = %updated_at,
            "mutated and persisted thread context through session manager"
        );
        Ok(result)
    }

    /// Load the current thread state snapshot, including persisted toolset metadata.
    pub async fn load_thread_state(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<StoredThreadState> {
        Ok(self
            .load_thread_context(locator)
            .await?
            .map(|thread_context| StoredThreadState {
                thread_context: Some(thread_context.clone()),
                thread: Some(thread_context.to_conversation_thread()),
                messages: thread_context.load_messages(),
                loaded_toolsets: thread_context.load_toolsets(),
                tool_events: thread_context.load_tool_events(),
            })
            .unwrap_or_default())
    }

    /// Return whether one external message was already fully processed for the target thread.
    pub async fn is_external_message_processed(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<bool> {
        Ok(self
            .store
            .load_external_message_record(locator, external_message_id)
            .await?
            .is_some())
    }

    /// Persist one completed external-message deduplication record without appending a new turn.
    pub async fn mark_external_message_processed(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
        turn_id: Option<Uuid>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let record = ExternalMessageDedupRecord {
            thread_id: locator.thread_id,
            external_message_id: external_message_id.to_string(),
            turn_id,
            completed_at,
        };
        self.store
            .save_external_message_record(locator, &record)
            .await
    }

    /// Store one completed turn for the provided channel/user/thread tuple.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::context::{ChatMessage, ChatMessageRole};
    /// use openjarvis::session::{SessionManager, ThreadLocator};
    /// use uuid::Uuid;
    ///
    /// let manager = SessionManager::new();
    /// let locator = ThreadLocator {
    ///     session_id: Uuid::new_v4(),
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    ///     external_thread_id: "default".to_string(),
    ///     thread_id: Uuid::new_v4(),
    /// };
    /// let _future = manager.store_turn(
    ///     &locator,
    ///     Some("msg_1".to_string()),
    ///     vec![ChatMessage::new(ChatMessageRole::User, "hello", Utc::now())],
    ///     Utc::now(),
    ///     Utc::now(),
    /// );
    /// ```
    pub async fn store_turn(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<Uuid> {
        self.store_turn_with_state(
            locator,
            external_message_id,
            messages,
            started_at,
            completed_at,
            Vec::new(),
            Vec::new(),
        )
        .await
    }

    /// Store one completed turn together with persisted thread tool runtime metadata.
    pub async fn store_turn_with_state(
        &self,
        locator: &ThreadLocator,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<Uuid> {
        self.store_turn_with_active_thread(
            locator,
            None,
            external_message_id,
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        )
        .await
    }

    /// Persist one updated thread context without appending a new turn.
    pub async fn store_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: ThreadContext,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let session_key = locator.session_key();
        let persisted = self
            .persist_thread_context_snapshot(locator, thread_context, updated_at)
            .await?;
        self.sync_thread_context_to_cache(&session_key, locator, &persisted, updated_at)
            .await;
        Ok(())
    }

    /// Store one completed turn after optionally replacing the current active thread context.
    pub async fn store_turn_with_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: Option<ThreadContext>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) -> SessionStoreResult<Uuid> {
        let session_key = locator.session_key();
        let _ = self
            .store
            .resolve_or_create_session(&session_key, completed_at)
            .await?;
        let mut thread_context = match thread_context {
            Some(thread_context) => thread_context,
            None => self.load_thread_context(locator).await?.unwrap_or_else(|| {
                ThreadContext::new(ThreadContextLocator::from(locator), completed_at)
            }),
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let turn_id = thread_context.store_turn(
            external_message_id.clone(),
            messages,
            started_at,
            completed_at,
        );
        let dedup_record =
            external_message_id
                .as_ref()
                .map(|message_id| ExternalMessageDedupRecord {
                    thread_id: locator.thread_id,
                    external_message_id: message_id.clone(),
                    turn_id: Some(turn_id),
                    completed_at,
                });
        thread_context = self
            .save_turn_snapshot_with_retry(
                locator,
                thread_context,
                completed_at,
                dedup_record.as_ref(),
            )
            .await?;

        self.sync_thread_context_to_cache(&session_key, locator, &thread_context, completed_at)
            .await;
        Ok(turn_id)
    }

    /// Store one completed turn after optionally replacing the current active thread history.
    pub async fn store_turn_with_active_thread(
        &self,
        locator: &ThreadLocator,
        active_thread: Option<ConversationThread>,
        external_message_id: Option<String>,
        messages: Vec<ChatMessage>,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        loaded_toolsets: Vec<String>,
        tool_events: Vec<ThreadToolEvent>,
    ) -> SessionStoreResult<Uuid> {
        let session_key = locator.session_key();
        let _ = self
            .store
            .resolve_or_create_session(&session_key, completed_at)
            .await?;
        let mut thread_context = self.load_thread_context(locator).await?.unwrap_or_else(|| {
            ThreadContext::new(ThreadContextLocator::from(locator), completed_at)
        });
        if let Some(active_thread) = active_thread {
            info!(
                thread_id = %locator.thread_id,
                turn_count = active_thread.turns.len(),
                "replacing active thread history before storing turn"
            );
            thread_context.overwrite_active_history_from_conversation_thread(&active_thread);
        }
        let turn_id = thread_context.store_turn_state(
            external_message_id.clone(),
            messages,
            started_at,
            completed_at,
            loaded_toolsets,
            tool_events,
        );
        let dedup_record =
            external_message_id
                .as_ref()
                .map(|message_id| ExternalMessageDedupRecord {
                    thread_id: locator.thread_id,
                    external_message_id: message_id.clone(),
                    turn_id: Some(turn_id),
                    completed_at,
                });
        thread_context = self
            .save_turn_snapshot_with_retry(
                locator,
                thread_context,
                completed_at,
                dedup_record.as_ref(),
            )
            .await?;

        self.sync_thread_context_to_cache(&session_key, locator, &thread_context, completed_at)
            .await;
        Ok(turn_id)
    }

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(key).cloned()
        }?;
        let mut threads = HashMap::with_capacity(session.threads.len());
        for (thread_id, thread_context) in session.threads {
            threads.insert(thread_id, thread_context.lock().await.clone());
        }
        Some(Session {
            id: session.id,
            key: session.key,
            threads,
            created_at: session.created_at,
            updated_at: session.updated_at.max(session.created_at),
        })
    }

    async fn save_turn_snapshot_with_retry(
        &self,
        locator: &ThreadLocator,
        thread_context: ThreadContext,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<ThreadContext> {
        match self
            .store
            .save_thread_context(&thread_context, updated_at, dedup_record)
            .await
        {
            Ok(new_revision) => {
                let mut thread_context = thread_context;
                thread_context.set_revision(new_revision);
                Ok(thread_context)
            }
            Err(SessionStoreError::RevisionConflict(_)) => {
                let Some(latest) = self.fetch_thread_context_from_store_only(locator).await? else {
                    return Err(SessionStoreError::Other(anyhow::anyhow!(
                        "thread `{}` disappeared during turn conflict recovery",
                        locator.thread_id
                    )));
                };
                let mut merged = merge_turn_conflict_resolution(latest, thread_context);
                let new_revision = self
                    .store
                    .save_thread_context(&merged, updated_at, dedup_record)
                    .await?;
                merged.set_revision(new_revision);
                Ok(merged)
            }
            Err(error) => Err(error),
        }
    }

    async fn cached_thread_handle(&self, locator: &ThreadLocator) -> Option<SharedThreadContext> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(&locator.session_key())
            .and_then(|session| session.threads.get(&locator.thread_id))
            .cloned()
    }

    async fn ensure_thread_handle(
        &self,
        locator: &ThreadLocator,
        session_record: Option<StoredSessionRecord>,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<SharedThreadContext> {
        if let Some(handle) = self.cached_thread_handle(locator).await {
            return Ok(handle);
        }

        let session_key = locator.session_key();
        let session_record = match session_record {
            Some(record) => record,
            None => {
                self.store
                    .resolve_or_create_session(&session_key, now)
                    .await?
            }
        };
        let persisted_thread = self.fetch_thread_context_from_store_only(locator).await?;
        let restored_from_store = persisted_thread.is_some();
        let thread_context = persisted_thread
            .unwrap_or_else(|| ThreadContext::new(ThreadContextLocator::from(locator), now));
        let handle = self
            .cache_thread_handle_if_absent(
                &session_key,
                session_record,
                locator.thread_id,
                Arc::new(Mutex::new(thread_context)),
                now,
            )
            .await;
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            restored_from_store,
            "ensured thread context handle in session cache"
        );
        Ok(handle)
    }

    async fn cache_thread_handle_if_absent(
        &self,
        session_key: &SessionKey,
        session_record: StoredSessionRecord,
        thread_id: Uuid,
        thread_context: SharedThreadContext,
        updated_at: DateTime<Utc>,
    ) -> SharedThreadContext {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession::from_record(session_record));
        session.updated_at = updated_at;
        Arc::clone(
            session
                .threads
                .entry(thread_id)
                .or_insert_with(|| thread_context),
        )
    }

    async fn sync_thread_context_to_cache(
        &self,
        session_key: &SessionKey,
        locator: &ThreadLocator,
        thread_context: &ThreadContext,
        updated_at: DateTime<Utc>,
    ) {
        let existing_handle = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .entry(session_key.clone())
                .or_insert_with(|| CachedSession {
                    id: locator.session_id,
                    key: session_key.clone(),
                    threads: HashMap::new(),
                    created_at: updated_at,
                    updated_at,
                });
            session.updated_at = updated_at;
            session.threads.get(&locator.thread_id).cloned()
        };

        if let Some(handle) = existing_handle {
            let mut cached_thread = handle.lock().await;
            *cached_thread = thread_context.clone();
            return;
        }

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession {
                id: locator.session_id,
                key: session_key.clone(),
                threads: HashMap::new(),
                created_at: updated_at,
                updated_at,
            });
        session.updated_at = updated_at;
        session
            .threads
            .entry(locator.thread_id)
            .or_insert_with(|| Arc::new(Mutex::new(thread_context.clone())));
    }

    async fn fetch_thread_context_from_store(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<(StoredSessionRecord, ThreadContext)>> {
        let session_key = locator.session_key();
        let Some(session_record) = self.store.load_session(&session_key).await? else {
            return Ok(None);
        };
        let Some(thread_context) = self.fetch_thread_context_from_store_only(locator).await? else {
            return Ok(None);
        };
        Ok(Some((session_record, thread_context)))
    }

    async fn fetch_thread_context_from_store_only(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<ThreadContext>> {
        let Some(mut thread_context) = self.store.load_thread_context(locator).await? else {
            return Ok(None);
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        Ok(Some(thread_context))
    }

    async fn persist_thread_context_snapshot(
        &self,
        locator: &ThreadLocator,
        mut thread_context: ThreadContext,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<ThreadContext> {
        let session_key = locator.session_key();
        let _ = self
            .store
            .resolve_or_create_session(&session_key, updated_at)
            .await?;
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        match self
            .store
            .save_thread_context(&thread_context, updated_at, None)
            .await
        {
            Ok(new_revision) => {
                thread_context.set_revision(new_revision);
                Ok(thread_context)
            }
            Err(SessionStoreError::RevisionConflict(_)) => {
                let Some(latest) = self.fetch_thread_context_from_store_only(locator).await? else {
                    return Err(SessionStoreError::Other(anyhow::anyhow!(
                        "thread `{}` disappeared during conflict recovery",
                        locator.thread_id
                    )));
                };
                let mut merged = merge_state_only_conflict_resolution(latest, &thread_context);
                let new_revision = self
                    .store
                    .save_thread_context(&merged, updated_at, None)
                    .await?;
                merged.set_revision(new_revision);
                Ok(merged)
            }
            Err(error) => Err(error),
        }
    }
}

fn merge_state_only_conflict_resolution(
    mut latest: ThreadContext,
    desired: &ThreadContext,
) -> ThreadContext {
    if desired.state.features.compact_enabled_override.is_some()
        || desired.state.features.auto_compact_override.is_some()
    {
        latest.state.features = desired.state.features.clone();
    }
    if !desired.request_context_system_messages().is_empty() {
        latest.state.request_context = desired.state.request_context.clone();
    }
    if !desired.state.approval.pending.is_empty() || !desired.state.approval.decisions.is_empty() {
        latest.state.approval = desired.state.approval.clone();
    }
    latest.rebind_locator(desired.locator.clone());
    latest
}

fn merge_turn_conflict_resolution(
    latest: ThreadContext,
    mut pending_turn_snapshot: ThreadContext,
) -> ThreadContext {
    pending_turn_snapshot.state.features = latest.state.features.clone();
    if pending_turn_snapshot
        .request_context_system_messages()
        .is_empty()
    {
        pending_turn_snapshot.state.request_context = latest.state.request_context.clone();
    }
    pending_turn_snapshot.state.approval = latest.state.approval.clone();
    pending_turn_snapshot.set_revision(latest.revision());
    pending_turn_snapshot
}
