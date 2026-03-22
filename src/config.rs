use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str = "你是 OpenJarvis，一个有帮助、可靠、简洁的 AI 助手。请直接回答用户问题；如需要工具，基于上下文发起工具调用。";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    server: ServerConfig,
    #[serde(flatten)]
    channels: ChannelConfig,
    llm: LlmConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        // 作用: 提供应用级默认配置，便于本地无配置文件时启动。
        // 参数: 无，默认值覆盖 server、channels 和 llm 三个子配置。
        Self {
            server: ServerConfig::default(),
            channels: ChannelConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        // 作用: 从环境变量指定路径或默认 config.yaml 加载应用配置。
        // 参数: 无，配置路径优先读取 OPENJARVIS_CONFIG。
        let path = env::var("OPENJARVIS_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        Self::from_path(path)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        // 作用: 从指定文件路径读取并解析 YAML 配置，不存在时返回默认配置。
        // 参数: path 为配置文件路径，可以是相对路径或绝对路径。
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config = serde_yaml::from_str::<Self>(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        Ok(config)
    }

    pub fn channel_config(&self) -> &ChannelConfig {
        // 作用: 暴露只读的 channel 子配置给启动流程或 router。
        // 参数: 无，返回配置中的 channels 视图。
        &self.channels
    }

    pub fn llm_config(&self) -> &LlmConfig {
        // 作用: 暴露只读的 llm 子配置给 agent 构造逻辑。
        // 参数: 无，返回配置中的 llm 视图。
        &self.llm
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        // 作用: 提供服务层默认配置。
        // 参数: 无，当前默认只设置监听地址。
        Self {
            bind: "0.0.0.0:3000".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ChannelConfig {
    feishu: FeishuConfig,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        // 作用: 提供 channel 聚合配置的默认值。
        // 参数: 无，当前默认只初始化 feishu 配置。
        Self {
            feishu: FeishuConfig::default(),
        }
    }
}

impl ChannelConfig {
    pub fn feishu_config(&self) -> &FeishuConfig {
        // 作用: 返回飞书 channel 的只读配置。
        // 参数: 无，当前只用于 channel 注册和运行时读取。
        &self.feishu
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FeishuConfig {
    pub mode: String,
    pub webhook_path: String,
    pub open_base_url: String,
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub encrypt_key: String,
    pub dry_run: bool,
    pub auto_start_sidecar: bool,
    pub node_bin: String,
    pub sidecar_script: String,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        // 作用: 提供飞书 channel 的默认配置，便于本地链路调试。
        // 参数: 无，默认启用 long_connection 且 dry_run=true。
        Self {
            mode: "long_connection".to_string(),
            webhook_path: "/webhook/feishu".to_string(),
            open_base_url: "https://open.feishu.cn".to_string(),
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: String::new(),
            dry_run: true,
            auto_start_sidecar: true,
            node_bin: "node".to_string(),
            sidecar_script: "scripts/feishu_ws_client.mjs".to_string(),
        }
    }
}

impl FeishuConfig {
    pub fn is_long_connection(&self) -> bool {
        // 作用: 判断当前飞书配置是否启用了长连接模式。
        // 参数: 无，判断依据为 mode 字段的别名集合。
        matches!(
            self.mode.as_str(),
            "long_connection" | "long-connection" | "long_connection_sdk" | "ws" | "websocket"
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub api_key_path: PathBuf,
    pub mock_response: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        // 作用: 提供 llm 的默认配置，默认走 mock provider。
        // 参数: 无，返回最小闭环运行所需的默认提示词和 mock 回复。
        Self {
            provider: "mock".to_string(),
            model: "mock-received".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            api_key_path: PathBuf::new(),
            mock_response: "[openjarvis][DEBUG] 测试回复".to_string(),
        }
    }
}
