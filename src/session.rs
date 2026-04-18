//! Session cache and thread-first storage orchestration.
//!
//! `SessionManager` now only resolves stable thread identities, owns the hot in-process thread
//! handle cache, and recovers thread snapshots from a pluggable store backend.

pub mod store;

use crate::model::IncomingMessage;
use crate::thread::{
    ChildThreadIdentity, SubagentSpawnMode, Thread, ThreadAgentKind, ThreadContextLocator,
    ThreadRuntime, ThreadSnapshotStore, derive_child_thread_id, derive_internal_thread_id,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::info;
use uuid::Uuid;

pub use store::{
    MemorySessionStore, SessionRevisionConflict, SessionStore, SessionStoreError,
    SessionStoreResult, SqliteSessionStore, StoredThreadRecord,
};

const OPENJARVIS_SESSION_ID_NAMESPACE: Uuid =
    Uuid::from_u128(0x2c427c19_1ec5_4637_8fb6_930f5d84ec48);

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

    /// Derive one stable runtime-only session id from the normalized session key.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::session::SessionKey;
    ///
    /// let key = SessionKey {
    ///     channel: "feishu".to_string(),
    ///     user_id: "ou_xxx".to_string(),
    /// };
    /// assert_eq!(key.derive_session_id(), key.derive_session_id());
    /// ```
    pub fn derive_session_id(&self) -> Uuid {
        let raw = format!("{}:{}", self.channel, self.user_id);
        Uuid::new_v5(&OPENJARVIS_SESSION_ID_NAMESPACE, raw.as_bytes())
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
    pub child_thread: Option<ChildThreadIdentity>,
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
    /// let session_id = SessionKey::from_incoming(&incoming).derive_session_id();
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
            child_thread: None,
        }
    }

    /// Build one child-thread locator that preserves the parent thread's channel identity.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::{SessionKey, ThreadLocator};
    /// use openjarvis::thread::SubagentSpawnMode;
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
    ///     external_thread_id: Some("chat_ext".to_string()),
    ///     received_at: Utc::now(),
    ///     metadata: json!({}),
    ///     attachments: Vec::new(),
    ///     reply_target: ReplyTarget {
    ///         receive_id: "oc_xxx".to_string(),
    ///         receive_id_type: "chat_id".to_string(),
    ///     },
    /// };
    /// let session_key = SessionKey::from_incoming(&incoming);
    /// let parent = ThreadLocator::new(
    ///     session_key.derive_session_id(),
    ///     &incoming,
    ///     "chat_ext",
    ///     session_key.derive_thread_id("chat_ext"),
    /// );
    ///
    /// let child = ThreadLocator::for_child(&parent, "browser", SubagentSpawnMode::Persist);
    /// assert_eq!(child.external_thread_id, parent.external_thread_id);
    /// assert_eq!(
    ///     child.child_thread.as_ref().map(|value| value.subagent_key.as_str()),
    ///     Some("browser")
    /// );
    /// ```
    pub fn for_child(
        parent: &ThreadLocator,
        subagent_key: impl Into<String>,
        spawn_mode: SubagentSpawnMode,
    ) -> Self {
        let child_thread =
            ChildThreadIdentity::new(parent.thread_id.to_string(), subagent_key, spawn_mode);
        Self {
            session_id: parent.session_id,
            channel: parent.channel.clone(),
            user_id: parent.user_id.clone(),
            external_thread_id: parent.external_thread_id.clone(),
            thread_id: derive_child_thread_id(
                &child_thread.parent_thread_id,
                &child_thread.subagent_key,
            ),
            child_thread: Some(child_thread),
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
        self.child_thread
            .as_ref()
            .map(ChildThreadIdentity::storage_key)
            .unwrap_or_else(|| self.session_key().thread_key(&self.external_thread_id))
    }
}

impl From<&ThreadLocator> for ThreadContextLocator {
    fn from(value: &ThreadLocator) -> Self {
        let mut locator = Self::new(
            Some(value.session_id.to_string()),
            value.channel.clone(),
            value.user_id.clone(),
            value.external_thread_id.clone(),
            value.thread_id.to_string(),
        );
        locator.child_thread = value.child_thread.clone();
        locator
    }
}

impl TryFrom<&ThreadContextLocator> for ThreadLocator {
    type Error = anyhow::Error;

    fn try_from(value: &ThreadContextLocator) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: Uuid::parse_str(
                value
                    .session_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("thread context locator missing session_id"))?,
            )?,
            channel: value.channel.clone(),
            user_id: value.user_id.clone(),
            external_thread_id: value.external_thread_id.clone(),
            thread_id: Uuid::parse_str(&value.thread_id)?,
            child_thread: value.child_thread.clone(),
        })
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

type SharedThreadContext = Arc<Mutex<Thread>>;

#[derive(Debug, Clone)]
struct CachedSession {
    id: Uuid,
    key: SessionKey,
    threads: HashMap<Uuid, SharedThreadContext>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone)]
struct BoundThreadStore {
    store: Arc<dyn SessionStore>,
}

impl std::fmt::Debug for BoundThreadStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundThreadStore").finish_non_exhaustive()
    }
}

#[async_trait]
impl ThreadSnapshotStore for BoundThreadStore {
    async fn save_thread_snapshot(
        &self,
        locator: &ThreadContextLocator,
        snapshot: &crate::thread::PersistedThreadSnapshot,
        expected_revision: u64,
    ) -> anyhow::Result<u64> {
        self.store
            .save_thread_snapshot(locator, snapshot, expected_revision)
            .await
    }
}

struct SessionManagerInner {
    sessions: Mutex<HashMap<SessionKey, CachedSession>>,
    store: Arc<dyn SessionStore>,
    bound_store: Arc<dyn ThreadSnapshotStore>,
    thread_runtime: RwLock<Option<Arc<ThreadRuntime>>>,
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
        let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
        let bound_store: Arc<dyn ThreadSnapshotStore> = Arc::new(BoundThreadStore {
            store: Arc::clone(&store),
        });
        Self {
            inner: Arc::new(SessionManagerInner {
                sessions: Mutex::new(HashMap::new()),
                store,
                bound_store,
                thread_runtime: RwLock::new(None),
            }),
        }
    }

    /// Create a session manager backed by the provided persistent store.
    pub async fn with_store(store: Arc<dyn SessionStore>) -> SessionStoreResult<Self> {
        store.initialize_schema().await?;
        let bound_store: Arc<dyn ThreadSnapshotStore> = Arc::new(BoundThreadStore {
            store: Arc::clone(&store),
        });
        Ok(Self {
            inner: Arc::new(SessionManagerInner {
                sessions: Mutex::new(HashMap::new()),
                store,
                bound_store,
                thread_runtime: RwLock::new(None),
            }),
        })
    }

    /// Install the thread runtime used by explicit create/reinitialize paths.
    pub fn install_thread_runtime(&self, thread_runtime: Arc<ThreadRuntime>) {
        let mut runtime = self
            .inner
            .thread_runtime
            .write()
            .expect("thread runtime lock should not be poisoned");
        *runtime = Some(thread_runtime);
    }

    fn resolve_thread_locator(&self, incoming: &IncomingMessage) -> ThreadLocator {
        let session_key = SessionKey::from_incoming(incoming);
        let external_thread_id = incoming.resolved_external_thread_id();
        let session_id = session_key.derive_session_id();
        let thread_id = session_key.derive_thread_id(&external_thread_id);
        ThreadLocator::new(session_id, incoming, external_thread_id, thread_id)
    }

    /// Resolve the external thread id on one incoming message and prepare a directly serviceable
    /// thread via the explicit create path.
    ///
    /// # 示例
    /// ```rust
    /// use chrono::Utc;
    /// use openjarvis::model::{IncomingMessage, ReplyTarget};
    /// use openjarvis::session::SessionManager;
    /// use openjarvis::thread::ThreadAgentKind;
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
    /// let _future = manager.create_thread(&incoming, ThreadAgentKind::Main);
    /// ```
    pub async fn create_thread(
        &self,
        incoming: &IncomingMessage,
        thread_agent_kind: ThreadAgentKind,
    ) -> SessionStoreResult<ThreadLocator> {
        let locator = self.resolve_thread_locator(incoming);
        self.create_thread_at(&locator, incoming.received_at, thread_agent_kind)
            .await
    }

    /// Prepare one thread from an explicit resolved locator.
    ///
    /// This path is reused by child-thread create flows so all thread preparation still runs
    /// through the same create lifecycle.
    pub async fn create_thread_at(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
        thread_agent_kind: ThreadAgentKind,
    ) -> SessionStoreResult<ThreadLocator> {
        let thread_key = locator.thread_key();

        info!(
            session_id = %locator.session_id,
            channel = %locator.channel,
            user_id = %locator.user_id,
            external_thread_id = %locator.external_thread_id,
            thread_key = %thread_key,
            thread_id = %locator.thread_id,
            parent_thread_id = ?locator
                .child_thread
                .as_ref()
                .map(|child| child.parent_thread_id.as_str()),
            subagent_key = ?locator
                .child_thread
                .as_ref()
                .map(|child| child.subagent_key.as_str()),
            spawn_mode = ?locator
                .child_thread
                .as_ref()
                .map(|child| child.spawn_mode.as_str()),
            thread_agent_kind = thread_agent_kind.as_str(),
            "resolved thread identity for create_thread"
        );

        let handle = self.create_or_restore_thread_handle(locator, now).await?;
        if let Some(child_thread) = locator.child_thread.clone() {
            handle
                .lock()
                .await
                .persist_child_thread_identity(child_thread)
                .await?;
        }
        self.initialize_thread_handle(&handle, thread_agent_kind)
            .await?;

        let session_key = locator.session_key();
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession {
                id: locator.session_id,
                key: session_key,
                threads: HashMap::new(),
                created_at: now,
                updated_at: now,
            });
        session.updated_at = now;
        Ok(locator.clone())
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread(&self, locator: &ThreadLocator) -> SessionStoreResult<Option<Thread>> {
        let Some(handle) = self
            .load_existing_thread_handle(locator, Utc::now())
            .await?
        else {
            info!(
                session_id = %locator.session_id,
                thread_id = %locator.thread_id,
                "load_thread did not find an existing thread"
            );
            return Ok(None);
        };
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            "loaded existing thread snapshot"
        );
        Ok(Some(handle.lock().await.clone()))
    }

    /// Lock one live thread context for in-process mutation.
    ///
    /// Cache miss 会先从 store 恢复；store miss 会显式返回缺失。
    pub async fn lock_thread(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<Option<OwnedMutexGuard<Thread>>> {
        let Some(handle) = self.load_existing_thread_handle(locator, now).await? else {
            info!(
                session_id = %locator.session_id,
                thread_id = %locator.thread_id,
                "lock_thread did not find an existing thread"
            );
            return Ok(None);
        };
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            "locked existing thread for mutation"
        );
        Ok(Some(handle.lock_owned().await))
    }

    /// Return all persisted child threads that belong to the provided parent thread.
    pub async fn list_child_threads(
        &self,
        parent_locator: &ThreadLocator,
    ) -> SessionStoreResult<Vec<StoredThreadRecord>> {
        self.inner.store.list_child_threads(parent_locator).await
    }

    /// Remove one persisted thread from cache and store.
    pub async fn remove_thread(&self, locator: &ThreadLocator) -> SessionStoreResult<bool> {
        let removed = self.inner.store.remove_thread_context(locator).await?;
        if !removed {
            return Ok(false);
        }

        let session_key = locator.session_key();
        let mut sessions = self.inner.sessions.lock().await;
        let mut drop_session = false;
        if let Some(session) = sessions.get_mut(&session_key) {
            session.threads.remove(&locator.thread_id);
            session.updated_at = Utc::now();
            drop_session = session.threads.is_empty();
        }
        if drop_session {
            sessions.remove(&session_key);
        }
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            "removed persisted thread from session manager"
        );
        Ok(true)
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

    async fn load_existing_thread_handle(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<Option<SharedThreadContext>> {
        if let Some(handle) = self.cached_thread_handle(locator).await {
            info!(
                session_id = %locator.session_id,
                thread_id = %locator.thread_id,
                "reused cached thread handle"
            );
            return Ok(Some(handle));
        }

        let restored = self.inner.store.load_thread_context(locator).await?;
        let Some(record) = restored else {
            info!(
                session_id = %locator.session_id,
                thread_id = %locator.thread_id,
                "thread was not found in cache or store"
            );
            return Ok(None);
        };
        let mut thread_context =
            Thread::from_persisted(record.locator, record.snapshot, record.revision);
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        thread_context.bind_store(Arc::clone(&self.inner.bound_store));
        let handle = self
            .cache_thread_handle_if_absent(locator, Arc::new(Mutex::new(thread_context)), now)
            .await;
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            "restored existing thread handle from store into session cache"
        );
        Ok(Some(handle))
    }

    async fn create_or_restore_thread_handle(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<SharedThreadContext> {
        if let Some(handle) = self.load_existing_thread_handle(locator, now).await? {
            return Ok(handle);
        }

        let mut thread_context = Thread::new(ThreadContextLocator::from(locator), now);
        thread_context.bind_store(Arc::clone(&self.inner.bound_store));
        let handle = self
            .cache_thread_handle_if_absent(locator, Arc::new(Mutex::new(thread_context)), now)
            .await;
        info!(
            session_id = %locator.session_id,
            thread_id = %locator.thread_id,
            "created new empty thread handle before initialization"
        );
        Ok(handle)
    }

    async fn cache_thread_handle_if_absent(
        &self,
        locator: &ThreadLocator,
        thread_context: SharedThreadContext,
        updated_at: DateTime<Utc>,
    ) -> SharedThreadContext {
        let session_key = locator.session_key();
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession {
                id: locator.session_id,
                key: session_key,
                threads: HashMap::new(),
                created_at: updated_at,
                updated_at,
            });
        session.updated_at = updated_at;
        Arc::clone(
            session
                .threads
                .entry(locator.thread_id)
                .or_insert_with(|| thread_context),
        )
    }

    async fn initialize_thread_handle(
        &self,
        handle: &SharedThreadContext,
        thread_agent_kind: ThreadAgentKind,
    ) -> SessionStoreResult<()> {
        let thread_runtime = self
            .inner
            .thread_runtime
            .read()
            .expect("thread runtime lock should not be poisoned")
            .clone();
        let Some(thread_runtime) = thread_runtime else {
            return Ok(());
        };

        let mut thread_context = handle.lock().await;
        thread_runtime
            .initialize_thread(&mut thread_context, thread_agent_kind)
            .await?;
        Ok(())
    }
}
