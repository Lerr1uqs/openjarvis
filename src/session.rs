//! Session cache and thread-first storage orchestration.
//!
//! `SessionManager` now only resolves stable thread identities, owns the hot in-process thread
//! handle cache, and recovers thread snapshots from a pluggable store backend.

pub mod store;

use crate::model::IncomingMessage;
use crate::thread::{
    Thread, ThreadContextLocator, ThreadRuntime, ThreadSnapshotStore, derive_internal_thread_id,
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

    /// Install the thread runtime used to initialize freshly resolved thread handles.
    pub fn install_thread_runtime(&self, thread_runtime: Arc<ThreadRuntime>) {
        let mut runtime = self
            .inner
            .thread_runtime
            .write()
            .expect("thread runtime lock should not be poisoned");
        *runtime = Some(thread_runtime);
    }

    /// Resolve the external thread id on one incoming message and create the thread on first
    /// sight.
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
        let session_id = session_key.derive_session_id();
        let thread_id = session_key.derive_thread_id(&external_thread_id);
        let locator = ThreadLocator::new(session_id, incoming, external_thread_id, thread_id);

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
            .ensure_thread_handle(&locator, incoming.received_at)
            .await?;
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| CachedSession {
                id: session_id,
                key: session_key,
                threads: HashMap::new(),
                created_at: incoming.received_at,
                updated_at: incoming.received_at,
            });
        session.updated_at = incoming.received_at;

        Ok(locator)
    }

    /// Load the current thread context snapshot for one channel/user/thread tuple.
    pub async fn load_thread_context(
        &self,
        locator: &ThreadLocator,
    ) -> SessionStoreResult<Option<Thread>> {
        let handle = self.ensure_thread_handle(locator, Utc::now()).await?;
        Ok(Some(handle.lock().await.clone()))
    }

    /// Lock one live thread context for in-process mutation.
    ///
    /// Cache miss 会先从 store 恢复；store miss 会创建一个新的空线程上下文。
    pub async fn lock_thread_context(
        &self,
        locator: &ThreadLocator,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<OwnedMutexGuard<Thread>> {
        let handle = self.ensure_thread_handle(locator, now).await?;
        Ok(handle.lock_owned().await)
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
        now: DateTime<Utc>,
    ) -> SessionStoreResult<SharedThreadContext> {
        if let Some(handle) = self.cached_thread_handle(locator).await {
            self.initialize_thread_handle_if_needed(&handle).await?;
            return Ok(handle);
        }

        let restored = self.inner.store.load_thread_context(locator).await?;
        let restored_from_store = restored.is_some();
        let mut thread_context = if let Some(record) = restored {
            Thread::from_persisted(record.locator, record.snapshot, record.revision)
        } else {
            Thread::new(ThreadContextLocator::from(locator), now)
        };
        thread_context.rebind_locator(ThreadContextLocator::from(locator));
        thread_context.bind_store(Arc::clone(&self.inner.bound_store));
        let handle = self
            .cache_thread_handle_if_absent(locator, Arc::new(Mutex::new(thread_context)), now)
            .await;
        self.initialize_thread_handle_if_needed(&handle).await?;
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

    async fn initialize_thread_handle_if_needed(
        &self,
        handle: &SharedThreadContext,
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
            .initialize_thread(&mut thread_context)
            .await?;
        Ok(())
    }
}
