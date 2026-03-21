pub mod feishu;

use crate::model::{IncomingMessage, OutgoingMessage};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct ChannelRegistration {
    pub incoming_tx: mpsc::Sender<IncomingMessage>,
    pub outgoing_rx: mpsc::Receiver<OutgoingMessage>,
}

#[async_trait]
pub trait Channel: Send + Sync {
    /// 作用: 返回当前 channel 在 router 中使用的唯一名称。
    /// 参数: 无，名称会写入统一消息模型中的 channel 字段。
    fn name(&self) -> &'static str;

    async fn on_start(&self) -> Result<()> {
        // 作用: 在 channel 正式启动前执行初始化检查或准备动作。
        // 参数: 无，默认实现不做额外处理。
        Ok(())
    }

    /// 作用: 启动当前 channel 的主循环，并接入 router 提供的双向通道。
    /// 参数: registration 包含当前 channel 的入站发送端和出站接收端。
    async fn start(self: Arc<Self>, registration: ChannelRegistration) -> Result<()>;

    async fn check_health(&self) -> Result<()> {
        // 作用: 在 channel 启动后做一次健康检查，确认基本能力正常。
        // 参数: 无，默认实现直接返回成功。
        Ok(())
    }

    #[allow(dead_code)]
    async fn on_stop(&self) -> Result<()> {
        // 作用: 在 channel 停止前执行资源清理逻辑。
        // 参数: 无，默认实现不做额外处理。
        Ok(())
    }
}
