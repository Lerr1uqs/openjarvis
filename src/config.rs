//! Configuration loading and default values for the application, channels, and LLM provider.

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
        Self {
            server: ServerConfig::default(),
            channels: ChannelConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from `OPENJARVIS_CONFIG` or `config.yaml`.
    pub fn load() -> Result<Self> {
        let path = env::var("OPENJARVIS_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        Self::from_path(path)
    }

    /// Load configuration from a specific YAML path, falling back to defaults when the file is missing.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
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

    /// Return the read-only channel configuration view.
    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channels
    }

    /// Return the read-only LLM configuration view.
    pub fn llm_config(&self) -> &LlmConfig {
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
        Self {
            feishu: FeishuConfig::default(),
        }
    }
}

impl ChannelConfig {
    /// Return the Feishu sub-configuration.
    pub fn feishu_config(&self) -> &FeishuConfig {
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
    /// Return whether the current Feishu mode should run with long connection semantics.
    pub fn is_long_connection(&self) -> bool {
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
