use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    pub user_message: String,
    pub assistant_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ConversationTurn {
    pub fn new(
        external_message_id: Option<String>,
        user_message: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        // 作用: 创建一个新的用户输入 turn，初始状态还没有 assistant 回复。
        // 参数: external_message_id 为平台原始消息 ID，user_message 为用户文本，started_at 为开始时间。
        Self {
            id: Uuid::new_v4(),
            external_message_id,
            user_message: user_message.into(),
            assistant_message: None,
            started_at,
            completed_at: None,
        }
    }

    pub fn complete(&mut self, assistant_message: impl Into<String>, completed_at: DateTime<Utc>) {
        // 作用: 为当前 turn 填充 assistant 回复并标记完成时间。
        // 参数: assistant_message 为回复文本，completed_at 为该轮完成时间。
        self.assistant_message = Some(assistant_message.into());
        self.completed_at = Some(completed_at);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationThread {
    pub id: String,
    pub turns: Vec<ConversationTurn>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationThread {
    pub fn new(id: impl Into<String>, now: DateTime<Utc>) -> Self {
        // 作用: 创建一个新的会话线程容器。
        // 参数: id 为线程标识，now 为创建和更新时间。
        Self {
            id: id.into(),
            turns: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn append_user_turn(
        &mut self,
        external_message_id: Option<String>,
        user_message: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Uuid {
        // 作用: 向线程末尾追加一条新的用户 turn。
        // 参数: external_message_id 为平台消息 ID，user_message 为用户文本，started_at 为该轮开始时间。
        let turn = ConversationTurn::new(external_message_id, user_message, started_at);
        let turn_id = turn.id;
        self.turns.push(turn);
        self.updated_at = started_at;
        turn_id
    }

    pub fn complete_turn(
        &mut self,
        turn_id: Uuid,
        assistant_message: impl Into<String>,
        completed_at: DateTime<Utc>,
    ) -> bool {
        // 作用: 根据 turn ID 补全 assistant 回复，并更新线程更新时间。
        // 参数: turn_id 为目标 turn，assistant_message 为回复文本，completed_at 为完成时间。
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == turn_id) else {
            return false;
        };

        turn.complete(assistant_message, completed_at);
        self.updated_at = completed_at;
        true
    }
}
