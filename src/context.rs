use crate::thread::ConversationThread;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: String,
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
            created_at,
        }
    }
}

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
