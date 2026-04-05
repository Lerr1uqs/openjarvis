//! Manual command-session verification binary for `exec_command` and `write_stdin`.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::Parser;
use openjarvis::{
    agent::{ToolCallRequest, ToolRegistry},
    thread::{Thread, ThreadContextLocator},
};
use serde_json::json;
use std::io::{self, Write};

/// Command-line options for the manual command-session verification binary.
#[derive(Debug, Clone, Parser)]
#[command(name = "command_session_manual")]
struct CommandSessionManualCli {
    /// Internal thread id used for the manual tool calls.
    #[arg(long, default_value = "command_session_manual")]
    thread_id: String,
    /// Yield time used by the initial `exec_command`.
    #[arg(long, default_value_t = 120)]
    exec_yield_time_ms: u64,
    /// Yield time used by each follow-up `write_stdin` poll.
    #[arg(long, default_value_t = 50)]
    poll_yield_time_ms: u64,
}

/// Run one interactive manual verification session against the command-session tools.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// let cli = clap::Parser::parse_from(["command_session_manual", "--thread-id", "demo"]);
/// # let _ = cli;
/// # Ok(())
/// # }
/// ```
async fn run_manual_session(cli: &CommandSessionManualCli) -> Result<()> {
    let registry = ToolRegistry::new();
    registry.register_builtin_tools().await?;

    let mut thread_context = build_thread(&cli.thread_id);
    let emit_command = build_emit_command();
    let started = registry
        .call_for_context(
            &mut thread_context,
            ToolCallRequest {
                name: "exec_command".to_string(),
                arguments: json!({
                    "cmd": emit_command,
                    "yield_time_ms": cli.exec_yield_time_ms,
                }),
            },
        )
        .await
        .context("failed to start the manual emit command through `exec_command`")?;

    println!("=== exec_command ===");
    println!("{}", started.content);
    let Some(session_id) = started.metadata["session_id"].as_str().map(str::to_owned) else {
        bail!("manual emit command did not stay running long enough to expose a session id");
    };

    println!();
    println!("输入任意字符后按回车，会触发一次空写 `write_stdin` 轮询。");
    println!("输入 `q` 或 `quit` 后按回车退出。");

    let stdin = io::stdin();
    loop {
        print!("manual-poll> ");
        io::stdout()
            .flush()
            .context("failed to flush the manual prompt")?;

        let mut line = String::new();
        let read_bytes = stdin
            .read_line(&mut line)
            .context("failed to read one manual poll line")?;
        if read_bytes == 0 {
            println!();
            println!("stdin 已关闭，结束手动验证。");
            break;
        }

        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") {
            println!("收到退出指令，结束手动验证。");
            break;
        }

        let polled = registry
            .call_for_context(
                &mut thread_context,
                ToolCallRequest {
                    name: "write_stdin".to_string(),
                    arguments: json!({
                        "session_id": session_id,
                        "chars": "",
                        "yield_time_ms": cli.poll_yield_time_ms,
                    }),
                },
            )
            .await
            .context("failed to poll the running emit command through `write_stdin`")?;

        println!("=== write_stdin ===");
        println!("{}", polled.content);
    }

    Ok(())
}

fn build_thread(thread_id: &str) -> Thread {
    Thread::new(
        ThreadContextLocator::new(
            None,
            "debug",
            "command_session_manual",
            thread_id,
            thread_id,
        ),
        Utc::now(),
    )
}

fn build_emit_command() -> String {
    #[cfg(windows)]
    {
        // Windows fallback keeps the loop bounded so the manual helper does not leave
        // an unbounded background process behind if the caller exits early.
        "$i = 0; for ($tick = 0; $tick -lt 600; $tick++) { Write-Host -NoNewline 'A '; $i += 1; if ($i -eq 10) { Write-Host ''; $i = 0 }; Start-Sleep -Milliseconds 500 }".to_string()
    }

    #[cfg(not(windows))]
    {
        // Bind the loop to the parent process lifetime so the helper exits automatically
        // when this manual binary exits.
        "parent_pid=$PPID; count=0; while kill -0 \"$parent_pid\" 2>/dev/null; do printf 'A '; count=$((count+1)); if [ \"$count\" -eq 10 ]; then printf '\\n'; count=0; fi; sleep 0.5; done".to_string()
    }
}

/// Main entrypoint for the manual command-session verification binary.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = CommandSessionManualCli::parse();
    if cli.thread_id.trim().is_empty() {
        bail!("`--thread-id` must not be blank");
    }
    run_manual_session(&cli).await
}
