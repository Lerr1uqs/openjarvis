use anyhow::{Context, Result, bail};
use chrono::Utc;
use openjarvis::{
    config::{AppConfig, DEFAULT_ASSISTANT_SYSTEM_PROMPT, LlmConfig},
    context::{ChatMessage, ChatMessageRole},
    llm::{LLMRequest, build_provider},
};
use std::{env, path::PathBuf};

struct Args {
    config_path: Option<PathBuf>,
    message: String,
    system_prompt: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 作用: 单独读取 LLM 配置并发起一次测试请求，用于验证 provider/base_url/api_key_path 是否可用。
    // 参数: 支持 `--config <path>` 覆盖配置文件路径，位置参数为测试消息，默认发送 `Hello`。
    let args = parse_args()?;
    let config = load_config(args.config_path.as_deref())?;
    let llm_config = config.llm_config();
    ensure_real_llm_config(llm_config)?;
    let system_prompt = resolve_system_prompt(&args);

    eprintln!(
        "llm_test: provider={}, model={}, base_url={}",
        llm_config.provider, llm_config.model, llm_config.base_url
    );
    if !llm_config.api_key_path.as_os_str().is_empty() {
        eprintln!(
            "llm_test: api_key_path={}",
            llm_config.api_key_path.display()
        );
    }
    eprintln!("llm_test: using_system_prompt={system_prompt}");

    let provider = build_provider(llm_config)?;
    let reply = provider
        .generate(LLMRequest {
            messages: build_test_messages(&system_prompt, &args.message),
            tools: Vec::new(),
        })
        .await
        .context("llm test request failed")?;

    let content = reply
        .message
        .map(|message| message.content)
        .context("llm test response did not contain assistant text")?;
    println!("{content}");
    Ok(())
}

fn parse_args() -> Result<Args> {
    // 作用: 解析 cargo bin 命令行参数。
    // 参数: 无，参数直接来自当前进程命令行。
    let mut args = env::args().skip(1);
    let mut config_path = None;
    let mut message = None;
    let mut system_prompt = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let Some(path) = args.next() else {
                    bail!("`--config` requires a file path");
                };
                config_path = Some(PathBuf::from(path));
            }
            "--system-prompt" => {
                let Some(prompt) = args.next() else {
                    bail!("`--system-prompt` requires a string value");
                };
                system_prompt = Some(prompt);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                if message.is_some() {
                    bail!("unexpected extra argument `{other}`");
                }
                message = Some(other.to_string());
            }
        }
    }

    Ok(Args {
        config_path,
        message: message.unwrap_or_else(|| "Hello".to_string()),
        system_prompt,
    })
}

fn load_config(config_path: Option<&std::path::Path>) -> Result<AppConfig> {
    // 作用: 从指定路径或默认位置读取配置文件。
    // 参数: config_path 为可选配置路径，未传入时回落到 AppConfig::load。
    match config_path {
        Some(path) => AppConfig::from_path(path),
        None => AppConfig::load(),
    }
}

fn ensure_real_llm_config(config: &LlmConfig) -> Result<()> {
    // 作用: 校验当前配置适合做真实 LLM 连通性测试，避免误走 mock provider。
    // 参数: config 为当前加载出的 llm 子配置。
    match config.provider.as_str() {
        "deepseek" | "openai" | "openai_compatible" => {}
        "mock" | "mock_llm" => {
            bail!("llm_test expects a real provider, but config.llm.provider is mock")
        }
        other => bail!("llm_test does not support provider `{other}`"),
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

fn resolve_system_prompt(args: &Args) -> String {
    // 作用: 为独立 llm 测试选择系统提示词；未显式传入时使用代码内置默认值。
    // 参数: args 为当前命令行参数。
    if let Some(prompt) = args.system_prompt.as_ref() {
        return prompt.clone();
    }

    DEFAULT_ASSISTANT_SYSTEM_PROMPT.to_string()
}

fn build_test_messages(system_prompt: &str, user_message: &str) -> Vec<ChatMessage> {
    // 作用: 为独立 llm 测试构造最小消息列表，保持与正式 provider 接口一致。
    // 参数: system_prompt 为测试使用的系统提示词，user_message 为用户输入文本。
    let mut messages = Vec::new();
    if !system_prompt.trim().is_empty() {
        messages.push(ChatMessage::new(
            ChatMessageRole::System,
            system_prompt,
            Utc::now(),
        ));
    }
    messages.push(ChatMessage::new(
        ChatMessageRole::User,
        user_message,
        Utc::now(),
    ));
    messages
}

fn print_help() {
    // 作用: 打印当前测试 bin 的使用说明。
    // 参数: 无，输出固定帮助文本。
    eprintln!(
        "Usage: cargo run --bin llm_test -- [--config <path>] [--system-prompt <text>] [message]"
    );
}
