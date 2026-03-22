use crate::thread::ConversationThread;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatMessageRole {
    System,
    Memory,
    User,
    Assistant,
    Tool,
    ToolResult,
}

impl ChatMessageRole {
    pub fn as_label(&self) -> &'static str {
        // 作用: 把内部消息角色枚举转换成渲染 prompt 时使用的标签文本。
        // 参数: 无，返回当前角色对应的稳定字符串。
        match self {
            Self::System => "system",
            Self::Memory => "memory",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::ToolResult => "tool_result",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ChatMessage {
    pub fn new(
        role: ChatMessageRole,
        content: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        // 作用: 创建一条结构化 chat message，用于 context 组织。
        // 参数: role 为消息角色，content 为文本内容，created_at 为创建时间。
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            created_at,
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ChatToolCall>) -> Self {
        // 作用: 为 assistant 消息追加原生 tool_calls，用于回放 tool use 历史。
        // 参数: tool_calls 为本条 assistant 消息携带的函数调用列表。
        self.tool_calls = tool_calls;
        self
    }

    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        // 作用: 为 tool 结果消息绑定对应的 tool_call_id。
        // 参数: tool_call_id 为模型发起该次工具调用时生成的唯一 ID。
        self.tool_call_id = Some(tool_call_id.into());
        self
    }
}

pub type Messages = Vec<ChatMessage>;
pub type ContextMessage = MessageContext;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageContext {
    pub system: Vec<ChatMessage>,
    pub memory: Vec<ChatMessage>,
    pub chat: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub system_prompt: String,
    pub user_message: String,
}

impl MessageContext {
    pub fn with_system_prompt(system_prompt: impl Into<String>) -> Self {
        // 作用: 创建一个带默认系统提示词的上下文容器。
        // 参数: system_prompt 为当前 agent 的系统提示词文本。
        let mut context = Self::default();
        context.push_system(system_prompt);
        context
    }

    pub fn push_system(&mut self, content: impl Into<String>) {
        // 作用: 向上下文的 system 区域追加一条系统消息。
        // 参数: content 为系统提示词内容。
        self.system.push(ChatMessage::new(
            ChatMessageRole::System,
            content,
            Utc::now(),
        ));
    }

    #[allow(dead_code)]
    pub fn push_memory(&mut self, content: impl Into<String>) {
        // 作用: 向上下文的 memory 区域追加一条记忆消息。
        // 参数: content 为命中的记忆文本内容。
        self.memory.push(ChatMessage::new(
            ChatMessageRole::Memory,
            content,
            Utc::now(),
        ));
    }

    pub fn extend_from_thread(&mut self, thread: &ConversationThread) {
        // 作用: 把 thread 中已有的 user/assistant turn 追加到 chat 区域。
        // 参数: thread 为当前会话线程，包含历史 turn 记录。
        for turn in &thread.turns {
            if !turn.messages.is_empty() {
                self.chat.extend(turn.messages.iter().cloned());
                continue;
            }

            self.chat.push(ChatMessage::new(
                ChatMessageRole::User,
                turn.user_message.clone(),
                turn.started_at,
            ));

            if let Some(assistant_message) = turn.assistant_message.as_ref() {
                self.chat.push(ChatMessage::new(
                    ChatMessageRole::Assistant,
                    assistant_message.clone(),
                    turn.completed_at.unwrap_or(turn.started_at),
                ));
            }
        }
    }

    pub fn as_messages(&self) -> Messages {
        // 作用: 按 system、memory、chat 顺序展开上下文，返回一份只读语义的消息副本给 LLM 或 agent loop 使用。
        // 参数: 无，返回当前上下文中的全部消息副本。
        let mut messages =
            Vec::with_capacity(self.system.len() + self.memory.len() + self.chat.len());
        messages.extend(self.system.iter().cloned());
        messages.extend(self.memory.iter().cloned());
        messages.extend(self.chat.iter().cloned());
        messages
    }

    pub fn render_for_llm(&self) -> RenderedPrompt {
        // 作用: 把结构化 context 渲染成当前 LLM provider 需要的 prompt 形式。
        // 参数: 无，当前会输出一个 system_prompt 和一个 user_message 字符串。
        let mut system_sections: Vec<String> =
            self.system.iter().map(|msg| msg.content.clone()).collect();
        if !self.memory.is_empty() {
            let memory_section = self
                .memory
                .iter()
                .map(|msg| format!("- {}", msg.content))
                .collect::<Vec<_>>()
                .join("\n");
            system_sections.push(format!("Memory:\n{memory_section}"));
        }

        let user_message = self
            .chat
            .iter()
            .map(|msg| format!("{}: {}", msg.role.as_label(), msg.content))
            .collect::<Vec<_>>()
            .join("\n");

        RenderedPrompt {
            system_prompt: system_sections.join("\n\n"),
            user_message,
        }
    }
}
