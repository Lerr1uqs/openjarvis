//! Session cache and persistent thread-context storage orchestration.
//!
//! `SessionManager` owns the hot in-process cache for active sessions and delegates durable
//! persistence to a pluggable `SessionStore` backend. The cache keeps one thread-level mutex for
//! each live `Thread`, while the store still persists detached snapshots so thread
//! recovery, tool state restoration, and external-message deduplication stay consistent across
//! restarts.

pub mod store;

use crate::model::IncomingMessage;
use crate::thread::{Thread, ThreadContextLocator, ThreadFinalizedTurn, derive_internal_thread_id};
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
    pub threads: HashMap<Uuid, Thread>,
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
    ) -> &mut Thread {
        let thread_id = Uuid::parse_str(&locator.thread_id)
            .expect("thread context locator should carry a UUID thread_id");
        self.threads
            .entry(thread_id)
            .or_insert_with(|| Thread::new(locator, now))
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

type SharedThreadContext = Arc<Mutex<Thread>>;

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

struct SessionManagerInner {
    sessions: Mutex<HashMap<SessionKey, CachedSession>>,
    store: Arc<dyn SessionStore>,
}

#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<SessionManagerInner>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager").finish_non_exhaustive()
    }
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
            inner: Arc::new(SessionManagerInner {
                sessions: Mutex::new(HashMap::new()),
                store: Arc::new(MemorySessionStore::new()),
            }),
        }
    }

    /// Create a session manager backed by the provided persistent store.
    pub async fn with_store(store: Arc<dyn SessionStore>) -> SessionStoreResult<Self> {
        store.initialize_schema().await?;
        Ok(Self {
            inner: Arc::new(SessionManagerInner {
                sessions: Mutex::new(HashMap::new()),
                store,
            }),
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
            .inner
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
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession::from_record(session_record.clone()));
        session.updated_at = now;

        Ok(locator)
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<Thread>> {
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
    ) -> SessionStoreResult<OwnedMutexGuard<Thread>> {
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
        F: FnOnce(&mut Thread) -> SessionStoreResult<R>,
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

    /// Persist one already locked live thread context and refresh the guard revision in place.
    pub async fn persist_locked_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: &mut Thread,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<()> {
        let persisted = self
            .persist_thread_context_snapshot(locator, thread_context.clone(), updated_at)
            .await?;
        *thread_context = persisted;
        self.touch_cached_session(&locator.session_key(), locator, updated_at)
            .await;
        Ok(())
    }

    /// Return whether one external message was already fully processed for the target thread.
    pub async fn is_external_message_processed(
        &self,
        locator: &ThreadLocator,
        external_message_id: &str,
    ) -> SessionStoreResult<bool> {
        Ok(self
            .inner
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
        self.inner
            .store
            .save_external_message_record(locator, &record)
            .await
    }

    /// Persist one finalized thread-owned turn snapshot and bind dedup to the same store write.
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn demo(
    /// #     manager: openjarvis::session::SessionManager,
    /// #     locator: openjarvis::session::ThreadLocator,
    /// #     finalized_turn: openjarvis::thread::ThreadFinalizedTurn,
    /// # ) -> openjarvis::session::SessionStoreResult<()> {
    /// manager.commit_finalized_turn(&locator, &finalized_turn).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn commit_finalized_turn(
        &self,
        locator: &ThreadLocator,
        finalized_turn: &ThreadFinalizedTurn,
    ) -> SessionStoreResult<Uuid> {
        let session_key = locator.session_key();
        let dedup_record = finalized_turn
            .external_message_id
            .as_ref()
            .map(|external_message_id| ExternalMessageDedupRecord {
                thread_id: locator.thread_id,
                external_message_id: external_message_id.clone(),
                turn_id: Some(finalized_turn.turn_id),
                completed_at: finalized_turn.completed_at,
            });
        let persisted = self
            .save_thread_snapshot(
                locator,
                finalized_turn.snapshot.clone(),
                finalized_turn.completed_at,
                dedup_record.as_ref(),
            )
            .await?;
        self.sync_thread_context_to_cache(
            &session_key,
            locator,
            &persisted,
            finalized_turn.completed_at,
        )
        .await;
        info!(
            thread_id = %locator.thread_id,
            turn_id = %finalized_turn.turn_id,
            has_dedup_record = dedup_record.is_some(),
            "committed finalized thread-owned turn"
        );
        Ok(finalized_turn.turn_id)
    }

    /// Persist one finalized turn while the caller owns the live thread lock.
    pub async fn commit_finalized_turn_locked(
        &self,
        locator: &ThreadLocator,
        thread_context: &mut Thread,
        finalized_turn: &ThreadFinalizedTurn,
    ) -> SessionStoreResult<Uuid> {
        let dedup_record = finalized_turn
            .external_message_id
            .as_ref()
            .map(|external_message_id| ExternalMessageDedupRecord {
                thread_id: locator.thread_id,
                external_message_id: external_message_id.clone(),
                turn_id: Some(finalized_turn.turn_id),
                completed_at: finalized_turn.completed_at,
            });
        let persisted = self
            .save_thread_snapshot(
                locator,
                finalized_turn.snapshot.clone(),
                finalized_turn.completed_at,
                dedup_record.as_ref(),
            )
            .await?;
        *thread_context = persisted;
        self.touch_cached_session(&locator.session_key(), locator, finalized_turn.completed_at)
            .await;
        info!(
            thread_id = %locator.thread_id,
            turn_id = %finalized_turn.turn_id,
            has_dedup_record = dedup_record.is_some(),
            "committed finalized thread-owned turn from locked thread context"
        );
        Ok(finalized_turn.turn_id)
    }

    /// Persist one updated thread context without appending a new turn.
    pub async fn store_thread_context(
        &self,
        locator: &ThreadLocator,
        thread_context: Thread,
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

    /// Return a cloned session snapshot for debugging or tests.
    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        let session = {
            let sessions = self.inner.sessions.lock().await;
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

    async fn cached_thread_handle(&self, locator: &ThreadLocator) -> Option<SharedThreadContext> {
        let sessions = self.inner.sessions.lock().await;
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
                self.inner
                    .store
                    .resolve_or_create_session(&session_key, now)
                    .await?
            }
        };
        let persisted_thread = self.fetch_thread_context_from_store_only(locator).await?;
        let restored_from_store = persisted_thread.is_some();
        let thread_context = persisted_thread
            .unwrap_or_else(|| Thread::new(ThreadContextLocator::from(locator), now));
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
        let mut sessions = self.inner.sessions.lock().await;
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
        thread_context: &Thread,
        updated_at: DateTime<Utc>,
    ) {
        let existing_handle = {
            let mut sessions = self.inner.sessions.lock().await;
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

        let mut sessions = self.inner.sessions.lock().await;
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
    ) -> SessionStoreResult<Option<(StoredSessionRecord, Thread)>> {
        let session_key = locator.session_key();
        let Some(session_record) = self.inner.store.load_session(&session_key).await? else {
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
    ) -> SessionStoreResult<Option<Thread>> {
        let Some(mut thread_context) = self.inner.store.load_thread_context(locator).await? else {
            return Ok(None);
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        Ok(Some(thread_context))
    }

    async fn persist_thread_context_snapshot(
        &self,
        locator: &ThreadLocator,
        thread_context: Thread,
        updated_at: DateTime<Utc>,
    ) -> SessionStoreResult<Thread> {
        self.save_thread_snapshot(locator, thread_context, updated_at, None)
            .await
    }

    async fn save_thread_snapshot(
        &self,
        locator: &ThreadLocator,
        mut thread_context: Thread,
        updated_at: DateTime<Utc>,
        dedup_record: Option<&ExternalMessageDedupRecord>,
    ) -> SessionStoreResult<Thread> {
        let session_key = locator.session_key();
        let _ = self
            .inner
            .store
            .resolve_or_create_session(&session_key, updated_at)
            .await?;
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        let new_revision = self
            .inner
            .store
            .save_thread_context(&thread_context, updated_at, dedup_record)
            .await?;
        thread_context.set_revision(new_revision);
        Ok(thread_context)
    }

    async fn touch_cached_session(
        &self,
        session_key: &SessionKey,
        locator: &ThreadLocator,
        updated_at: DateTime<Utc>,
    ) {
        let mut sessions = self.inner.sessions.lock().await;
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
    }
}
