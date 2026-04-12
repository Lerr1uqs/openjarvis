//! Connectivity verification binary dedicated to one OpenAI Responses provider.

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

/// Command-line options for the fixed Responses `ping -> pong` probe.
#[derive(Debug, Clone, Parser)]
#[command(name = "openai_responses_ping")]
struct OpenaiResponsesPingCli {
    /// Optional config file path. Defaults to `OPENJARVIS_CONFIG` or `config.yaml`.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Optional provider name override. When set, it replaces the active provider selector.
    #[arg(long)]
    provider: Option<String>,
}

/// Run one fixed connectivity probe against an OpenAI Responses provider.
async fn run_ping(cli: &OpenaiResponsesPingCli) -> Result<()> {
    let config = load_config(cli.config.as_deref())?;
    let selected_config =
        select_provider_config(config.llm_config().clone(), cli.provider.as_deref())?;
    ensure_responses_provider_config(&selected_config)?;
    let resolved_provider = selected_config
        .resolve_active_provider()
        .context("openai_responses_ping requires one resolved active provider")?;

    eprintln!(
        "openai_responses_ping: protocol={}, provider={}, model={}, base_url={}",
        resolved_provider.effective_protocol(),
        resolved_provider.name,
        resolved_provider.model,
        resolved_provider.base_url
    );
    if !resolved_provider.api_key_path.as_os_str().is_empty() {
        eprintln!(
            "openai_responses_ping: api_key_path={}",
            resolved_provider.api_key_path.display()
        );
    }
    eprintln!("openai_responses_ping: sending fixed ping request");

    let provider = build_provider(&selected_config)?;
    let reply = provider
        .generate(LLMRequest {
            messages: build_ping_messages(),
            tools: Vec::new(),
        })
        .await
        .context("openai responses ping request failed")?;

    let content = reply
        .items
        .into_iter()
        .filter(|item| item.role == ChatMessageRole::Assistant)
        .map(|message| message.content)
        .find(|content| !content.trim().is_empty())
        .context("openai responses ping response did not contain assistant text")?;
    ensure_expected_pong(&content)?;

    eprintln!("openai_responses_ping: connectivity probe succeeded");
    println!("{EXPECTED_PONG}");
    Ok(())
}

fn load_config(config_path: Option<&Path>) -> Result<AppConfig> {
    match config_path {
        Some(path) => AppConfig::from_path(path),
        None => AppConfig::load(),
    }
}

fn select_provider_config(
    mut config: LLMConfig,
    provider_override: Option<&str>,
) -> Result<LLMConfig> {
    if let Some(provider_name) = provider_override {
        let provider_name = provider_name.trim();
        if provider_name.is_empty() {
            bail!("--provider must not be blank");
        }
        if !config.providers.is_empty() {
            config.active_provider = Some(provider_name.to_string());
        } else {
            let resolved = config
                .resolve_active_provider()
                .context("openai_responses_ping requires one resolved active provider")?;
            if resolved.name != provider_name {
                bail!(
                    "configured provider `{}` does not match requested --provider `{provider_name}`",
                    resolved.name
                );
            }
        }
    }

    Ok(config)
}

fn ensure_responses_provider_config(config: &LLMConfig) -> Result<()> {
    let resolved = config
        .resolve_active_provider()
        .context("openai_responses_ping requires one resolved active provider")?;
    if resolved.effective_protocol() != "openai_responses" {
        bail!(
            "openai_responses_ping expects an openai_responses provider, but resolved protocol is `{}`",
            resolved.effective_protocol()
        );
    }
    if resolved.model.trim().is_empty() {
        bail!("llm.model is required");
    }
    if resolved.base_url.trim().is_empty() {
        bail!("llm.base_url is required");
    }
    if resolved.api_key.trim().is_empty() && resolved.api_key_path.as_os_str().is_empty() {
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
        "openai responses connectivity check expected `{}`, but got `{}`",
        EXPECTED_PONG,
        normalized
    )
}

/// Main entrypoint for the Responses connectivity probe binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = OpenaiResponsesPingCli::parse();
    run_ping(&cli).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::collections::HashMap;

    #[test]
    fn cli_accepts_provider_override() {
        let cli = OpenaiResponsesPingCli::parse_from([
            "openai_responses_ping",
            "--provider",
            "dashscope",
        ]);

        assert_eq!(cli.provider.as_deref(), Some("dashscope"));
    }

    #[test]
    fn ensure_expected_pong_accepts_trimmed_exact_reply() {
        ensure_expected_pong(" pong\n").expect("trimmed pong should pass");
    }

    #[test]
    fn provider_override_replaces_active_provider_selector() {
        let config = LLMConfig {
            active_provider: Some("zai".to_string()),
            providers: HashMap::from([
                (
                    "zai".to_string(),
                    openjarvis::config::LLMProviderProfileConfig {
                        protocol: "openai-compatible".to_string(),
                        model: "glm-5".to_string(),
                        base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
                        api_key_path: PathBuf::from("~/.zai.apikey"),
                        ..openjarvis::config::LLMProviderProfileConfig::default()
                    },
                ),
                (
                    "dashscope".to_string(),
                    openjarvis::config::LLMProviderProfileConfig {
                        protocol: "openai-response".to_string(),
                        model: "qwen3.6-plus".to_string(),
                        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1/responses"
                            .to_string(),
                        api_key_path: PathBuf::from("~/.qwen.apikey"),
                        ..openjarvis::config::LLMProviderProfileConfig::default()
                    },
                ),
            ]),
            ..LLMConfig::default()
        };

        let selected = select_provider_config(config, Some("dashscope"))
            .expect("provider override should succeed");
        let resolved = selected
            .resolve_active_provider()
            .expect("overridden provider should resolve");

        assert_eq!(resolved.name, "dashscope");
        assert_eq!(resolved.effective_protocol(), "openai_responses");
    }
}
