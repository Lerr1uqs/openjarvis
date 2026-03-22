use crate::context::{ChatMessage, ChatMessageRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: Uuid,
    pub external_message_id: Option<String>,
    pub user_message: String,
    pub assistant_message: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
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
        let user_message = user_message.into();
        Self {
            id: Uuid::new_v4(),
            external_message_id,
            user_message: user_message.clone(),
            assistant_message: None,
            messages: vec![ChatMessage::new(
                ChatMessageRole::User,
                user_message,
                started_at,
            )],
            started_at,
            completed_at: None,
        }
    }

    pub fn complete(&mut self, assistant_message: impl Into<String>, completed_at: DateTime<Utc>) {
        // 作用: 为当前 turn 填充 assistant 回复并标记完成时间。
        // 参数: assistant_message 为回复文本，completed_at 为该轮完成时间。
        let assistant_message = assistant_message.into();
        self.complete_with_messages(
            vec![ChatMessage::new(
                ChatMessageRole::Assistant,
                assistant_message,
                completed_at,
            )],
            completed_at,
        );
    }

    pub fn complete_with_messages(
        &mut self,
        messages: Vec<ChatMessage>,
        completed_at: DateTime<Utc>,
    ) {
        // 作用: 为当前 turn 追加一组完整的 assistant/tool 历史消息并标记完成时间。
        // 参数: messages 为本轮生成的消息轨迹，completed_at 为该轮完成时间。
        self.assistant_message = select_final_assistant_message(&messages);
        self.messages.extend(messages);
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

    pub fn complete_turn_with_messages(
        &mut self,
        turn_id: Uuid,
        messages: Vec<ChatMessage>,
        completed_at: DateTime<Utc>,
    ) -> bool {
        // 作用: 根据 turn ID 追加一组结构化消息轨迹，并更新线程更新时间。
        // 参数: turn_id 为目标 turn，messages 为本轮 assistant/tool 历史，completed_at 为完成时间。
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == turn_id) else {
            return false;
        };

        turn.complete_with_messages(messages, completed_at);
        self.updated_at = completed_at;
        true
    }
}

fn select_final_assistant_message(messages: &[ChatMessage]) -> Option<String> {
    // 作用: 从一轮结构化消息中挑出最终 assistant 回复，兼容 tool-call 前导文本和最终回答。
    // 参数: messages 为本轮 assistant/tool 历史消息列表。
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role == ChatMessageRole::Assistant
                && message.tool_calls.is_empty()
                && !message.content.trim().is_empty()
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .find(|message| message.role == ChatMessageRole::Assistant)
        })
        .map(|message| message.content.clone())
}
