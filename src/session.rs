use crate::context::MessageContext;
use crate::model::IncomingMessage;
use crate::thread::ConversationThread;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionKey {
    pub channel: String,
    pub user_id: String,
}

impl SessionKey {
    pub fn from_incoming(incoming: &IncomingMessage) -> Self {
        // 作用: 从统一入站消息中提取 session 的唯一键。
        // 参数: incoming 为标准化后的用户消息，包含 channel 和 user_id。
        Self {
            channel: incoming.channel.clone(),
            user_id: incoming.user_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub threads: HashMap<String, ConversationThread>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    pub fn new(now: DateTime<Utc>) -> Self {
        // 作用: 创建一个新的 session 容器。
        // 参数: now 为 session 的初始更新时间。
        Self {
            threads: HashMap::new(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingTurn {
    pub session_key: SessionKey,
    pub thread_id: String,
    pub turn_id: Uuid,
    pub context: MessageContext,
}

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionKey, Session>>,
}

impl SessionManager {
    pub fn new() -> Self {
        // 作用: 创建内存态 session manager，用于维护当前进程内的会话状态。
        // 参数: 无，内部会初始化空的 session 映射。
        Self::default()
    }

    pub async fn begin_turn(&self, incoming: &IncomingMessage, system_prompt: &str) -> PendingTurn {
        // 作用: 为当前入站消息找到或创建 session/thread，并生成本轮待执行上下文。
        // 参数: incoming 为用户消息，system_prompt 为当前 agent 使用的系统提示词。
        let now = Utc::now();
        let session_key = SessionKey::from_incoming(incoming);
        let thread_id = resolve_thread_id(incoming);
        let received_at = incoming.received_at;
        let external_message_id = incoming.external_message_id.clone();
        let user_message = incoming.content.clone();

        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(|| Session::new(now));
        let thread = session
            .threads
            .entry(thread_id.clone())
            .or_insert_with(|| ConversationThread::new(thread_id.clone(), now));
        let turn_id = thread.append_user_turn(external_message_id, user_message, received_at);
        session.updated_at = now;

        let mut context = MessageContext::with_system_prompt(system_prompt.to_string());
        context.extend_from_thread(thread);

        PendingTurn {
            session_key,
            thread_id,
            turn_id,
            context,
        }
    }

    pub async fn complete_turn(&self, pending: &PendingTurn, assistant_message: &str) {
        // 作用: 在 LLM 返回后把 assistant 回复补写回对应的 turn。
        // 参数: pending 为 begin_turn 返回的挂起轮次，assistant_message 为回复文本。
        let mut sessions = self.sessions.write().await;
        let Some(session) = sessions.get_mut(&pending.session_key) else {
            return;
        };
        let Some(thread) = session.threads.get_mut(&pending.thread_id) else {
            return;
        };

        thread.complete_turn(pending.turn_id, assistant_message.to_string(), Utc::now());
        session.updated_at = Utc::now();
    }

    pub async fn get_session(&self, key: &SessionKey) -> Option<Session> {
        // 作用: 查询当前内存中的 session 快照，主要用于调试和测试。
        // 参数: key 为 channel + user_id 组成的 session 唯一键。
        let sessions = self.sessions.read().await;
        sessions.get(key).cloned()
    }
}

fn resolve_thread_id(incoming: &IncomingMessage) -> String {
    // 作用: 解析当前消息所属线程，没有线程 ID 时回落到 default。
    // 参数: incoming 为统一入站消息，可能带平台原生 thread_id。
    incoming
        .thread_id
        .clone()
        .filter(|thread_id| !thread_id.trim().is_empty())
        .unwrap_or_else(|| "default".to_string())
}
