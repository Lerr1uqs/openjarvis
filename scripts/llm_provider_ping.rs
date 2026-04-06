//! Connectivity verification binary for the configured LLM provider.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::Parser;
use openjarvis::{
    config::{AppConfig, LLMConfig},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMRequest, build_provider},
};
use std::path::{Path, PathBuf};

const EXPECTED_PONG: &str = "pong";
const CONNECTIVITY_SYSTEM_PROMPT: &str = "You are a connectivity probe. Reply with exactly pong in lowercase ASCII. Do not output any other words, punctuation, code fences, or explanations.";
const CONNECTIVITY_USER_MESSAGE: &str = "ping";

/// Command-line options for the fixed `ping -> pong` connectivity probe.
#[derive(Debug, Clone, Parser)]
#[command(name = "llm_provider_ping")]
struct LlmProviderPingCli {
    /// Optional config file path. Defaults to `OPENJARVIS_CONFIG` or `config.yaml`.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Run one fixed connectivity probe against the configured real LLM provider.
async fn run_ping(cli: &LlmProviderPingCli) -> Result<()> {
    let config = load_config(cli.config.as_deref())?;
    let llm_config = config.llm_config();
    ensure_connectivity_provider_config(llm_config)?;

    eprintln!(
        "llm_provider_ping: protocol={}, provider={}, model={}, base_url={}",
        llm_config.effective_protocol(),
        llm_config.provider,
        llm_config.model,
        llm_config.base_url
    );
    if !llm_config.api_key_path.as_os_str().is_empty() {
        eprintln!(
            "llm_provider_ping: api_key_path={}",
            llm_config.api_key_path.display()
        );
    }
    eprintln!("llm_provider_ping: sending fixed ping request");

    let provider = build_provider(llm_config)?;
    let reply = provider
        .generate(LLMRequest {
            messages: build_ping_messages(),
            tools: Vec::new(),
        })
        .await
        .context("llm provider ping request failed")?;

    let content = reply
        .message
        .map(|message| message.content)
        .context("llm provider ping response did not contain assistant text")?;
    ensure_expected_pong(&content)?;

    eprintln!("llm_provider_ping: connectivity probe succeeded");
    println!("{EXPECTED_PONG}");
    Ok(())
}

fn load_config(config_path: Option<&Path>) -> Result<AppConfig> {
    match config_path {
        Some(path) => AppConfig::from_path(path),
        None => AppConfig::load(),
    }
}

fn ensure_connectivity_provider_config(config: &LLMConfig) -> Result<()> {
    match config.effective_protocol() {
        "openai_compatible" => {}
        "mock" => {
            bail!("llm_provider_ping expects a real provider, but llm.protocol resolved to mock")
        }
        "anthropic" => bail!("llm_provider_ping does not support anthropic providers yet"),
        other => bail!("llm_provider_ping does not support llm protocol `{other}`"),
    }

    if config.model.trim().is_empty() {
        bail!("llm.model is required");
    }
    if config.base_url.trim().is_empty() {
        bail!("llm.base_url is required");
    }
    if config.api_key.trim().is_empty() && config.api_key_path.as_os_str().is_empty() {
        bail!("llm.api_key or llm.api_key_path is required");
    }

    Ok(())
}

fn build_ping_messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage::new(
            ChatMessageRole::System,
            CONNECTIVITY_SYSTEM_PROMPT,
            Utc::now(),
        ),
        ChatMessage::new(ChatMessageRole::User, CONNECTIVITY_USER_MESSAGE, Utc::now()),
    ]
}

fn ensure_expected_pong(content: &str) -> Result<()> {
    let normalized = content.trim();
    if normalized == EXPECTED_PONG {
        return Ok(());
    }

    bail!(
        "llm connectivity check expected `{}`, but got `{}`",
        EXPECTED_PONG,
        normalized
    )
}

/// Main entrypoint for the provider connectivity probe binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = LlmProviderPingCli::parse();
    run_ping(&cli).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_defaults_to_workspace_config_resolution() {
        let cli = LlmProviderPingCli::parse_from(["llm_provider_ping"]);

        assert!(cli.config.is_none());
    }

    #[test]
    fn cli_accepts_explicit_config_path() {
        let cli =
            LlmProviderPingCli::parse_from(["llm_provider_ping", "--config", "tmp/config.yaml"]);

        assert_eq!(cli.config, Some(PathBuf::from("tmp/config.yaml")));
    }

    #[test]
    fn ensure_expected_pong_accepts_trimmed_exact_reply() {
        // 测试场景: provider 返回前后带空白的 `pong` 时，脚本仍应判定为连通成功。
        ensure_expected_pong("  pong\n").expect("trimmed pong should pass");
    }

    #[test]
    fn ensure_expected_pong_rejects_non_exact_reply() {
        // 测试场景: provider 如果返回额外文本，脚本必须立即失败，避免把弱约束回复误判为成功。
        let error =
            ensure_expected_pong("pong.").expect_err("reply with extra punctuation should fail");

        assert!(
            error
                .to_string()
                .contains("llm connectivity check expected `pong`")
        );
    }

    #[test]
    fn zai_provider_is_treated_as_supported_openai_compatible_backend() {
        // 测试场景: 当 protocol 明确为 openai 时，脚本必须接受任意自定义 provider 名称。
        let config = LLMConfig {
            protocol: "openai".to_string(),
            provider: "zai".to_string(),
            model: "glm-5".to_string(),
            base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
            api_key: "test-key".to_string(),
            ..LLMConfig::default()
        };

        ensure_connectivity_provider_config(&config)
            .expect("zai provider should be accepted by connectivity probe");
    }
}
