use crate::config::LlmConfig;
use crate::context::RenderedPrompt;
use crate::llm::{LlmProvider, LlmRequest, build_provider};
use crate::model::{IncomingMessage, OutgoingMessage};
use crate::session::SessionManager;
use anyhow::Result;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub struct AgentWorker {
    llm: Arc<dyn LlmProvider>,
    system_prompt: String,
    sessions: SessionManager,
}

impl AgentWorker {
    pub fn new(llm: Arc<dyn LlmProvider>, system_prompt: impl Into<String>) -> Self {
        // 作用: 创建 agent worker，并注入当前会话管理器和默认系统提示词。
        // 参数: llm 为当前 agent 使用的模型提供者，system_prompt 为系统提示词模板。
        Self {
            llm,
            system_prompt: system_prompt.into(),
            sessions: SessionManager::new(),
        }
    }

    pub fn from_config(config: &LlmConfig) -> Result<Self> {
        // 作用: 根据配置自动构造 agent 所需的 LLM provider 和系统提示词。
        // 参数: config 为 llm 子配置，决定 provider 类型和提示词。
        Ok(Self::new(
            build_provider(config)?,
            config.system_prompt.clone(),
        ))
    }

    pub async fn handle_message(
        &self,
        incoming: IncomingMessage,
    ) -> Result<Option<OutgoingMessage>> {
        // 作用: 处理一条统一入站消息，补全 session/context 后调用 LLM 并生成回复。
        // 参数: incoming 为 channel 已标准化后的用户消息。
        let pending_turn = self
            .sessions
            .begin_turn(&incoming, &self.system_prompt)
            .await;
        let RenderedPrompt {
            system_prompt,
            user_message,
        } = pending_turn.context.render_for_llm();
        let reply = self
            .llm
            .generate(LlmRequest {
                system_prompt,
                user_message,
            })
            .await?;
        self.sessions.complete_turn(&pending_turn, &reply).await;

        let outgoing_thread_id = incoming
            .thread_id
            .clone()
            .or_else(|| Some(pending_turn.thread_id.clone()));
        let source_message_id = incoming.external_message_id.clone();

        Ok(Some(OutgoingMessage {
            id: Uuid::new_v4(),
            channel: incoming.channel,
            content: reply,
            thread_id: outgoing_thread_id,
            metadata: json!({
                "source_message_id": source_message_id,
                "session_channel": pending_turn.session_key.channel,
                "session_user_id": pending_turn.session_key.user_id,
                "session_thread_id": pending_turn.thread_id,
            }),
            reply_to_message_id: incoming.external_message_id,
            target: incoming.reply_target,
        }))
    }
}
