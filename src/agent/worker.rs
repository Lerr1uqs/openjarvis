use super::{
    agent_loop::{AgentEventSender, AgentLoop, AgentLoopInput, AgentLoopOutput},
    runtime::AgentRuntime,
};
use crate::config::{DEFAULT_ASSISTANT_SYSTEM_PROMPT, LlmConfig};
use crate::llm::{LLMProvider, build_provider};
use crate::model::IncomingMessage;
use crate::session::SessionManager;
use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AgentWorker {
    agent_loop: AgentLoop,
    system_prompt: String,
    sessions: SessionManager,
}

impl AgentWorker {
    pub fn new(llm: Arc<dyn LLMProvider>, system_prompt: impl Into<String>) -> Self {
        // 作用: 创建 agent worker，并注入默认 runtime、会话管理器和系统提示词。
        // 参数: llm 为当前 agent 使用的模型提供者，system_prompt 为系统提示词模板。
        Self::with_runtime(llm, system_prompt, AgentRuntime::new())
    }

    pub fn with_runtime(
        llm: Arc<dyn LLMProvider>,
        system_prompt: impl Into<String>,
        runtime: AgentRuntime,
    ) -> Self {
        // 作用: 用指定 runtime 创建 agent worker，便于后续注入 hooks、tools 和 mcp。
        // 参数: llm 为模型提供者，system_prompt 为系统提示词，runtime 为 agent 运行时。
        Self {
            agent_loop: AgentLoop::new(llm, runtime),
            system_prompt: system_prompt.into(),
            sessions: SessionManager::new(),
        }
    }

    pub fn from_config(config: &LlmConfig) -> Result<Self> {
        // 作用: 根据配置自动构造 agent 所需的 LLM provider 和系统提示词。
        // 参数: config 为 llm 子配置，决定 provider 类型；系统提示词当前固定使用内置默认值。
        Ok(Self::new(
            build_provider(config)?,
            DEFAULT_ASSISTANT_SYSTEM_PROMPT,
        ))
    }

    pub fn runtime(&self) -> &AgentRuntime {
        // 作用: 暴露当前 worker 绑定的 runtime，供外部注册 hooks、tools 和 mcp。
        // 参数: 无，返回当前 worker 内部持有的 runtime 引用。
        self.agent_loop.runtime()
    }
    pub async fn handle_message(
        &self,
        incoming: IncomingMessage,
        router_tx: mpsc::Sender<crate::model::OutgoingMessage>,
    ) -> Result<AgentLoopOutput> {
        // 作用: 处理一条统一入站消息，补全 session/context 后通过 agent loop 生成回复。
        // 参数: incoming 为 channel 已标准化后的用户消息。
        let pending_turn = self
            .sessions
            .begin_turn(&incoming, &self.system_prompt)
            .await;
        let loop_output = self
            .agent_loop
            .run(
                AgentLoopInput {
                    channel: incoming.channel.clone(),
                    user_id: incoming.user_id.clone(),
                    thread_id: pending_turn.thread_id.clone(),
                    event_tx: AgentEventSender::new(
                        router_tx,
                        incoming.channel.clone(),
                        pending_turn.thread_id.clone(),
                        incoming.external_message_id.clone(),
                        incoming.reply_target.clone(),
                        pending_turn.session_key.channel.clone(),
                        pending_turn.session_key.user_id.clone(),
                        pending_turn.thread_id.clone(),
                    ),
                },
                &pending_turn.context,
            )
            .await?;
        self.sessions
            .complete_turn_with_messages(
                &pending_turn,
                loop_output.turn_messages.clone(),
                Utc::now(),
            )
            .await;

        Ok(loop_output)
    }
}
