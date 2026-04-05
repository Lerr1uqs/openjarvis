//! Process spawning helpers shared by command-session tools and the legacy `bash` wrapper.

use super::session::CommandExecutionRequest;
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    process::Stdio,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
    time::{Duration, timeout},
};
use tracing::{debug, warn};

const PTY_DEFAULT_ROWS: u16 = 24;
const PTY_DEFAULT_COLS: u16 = 80;
#[cfg(windows)]
const WINDOWS_UTF8_POWERSHELL_PREFIX: &str = "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false); chcp.com 65001 > $null;";

#[derive(Debug)]
pub(crate) enum ProcessEvent {
    Output(String),
    Exit(i32),
}

pub(crate) struct SpawnedCommand {
    pub(crate) input_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(crate) event_rx: mpsc::UnboundedReceiver<ProcessEvent>,
}

#[derive(Debug)]
pub(crate) struct LegacyShellCommandResult {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) status_code: Option<i32>,
    pub(crate) timed_out: bool,
}

#[derive(Debug, Clone)]
struct ShellCommandSpec {
    program: String,
    args: Vec<String>,
}

pub(crate) async fn spawn_command(request: &CommandExecutionRequest) -> Result<SpawnedCommand> {
    if request.tty {
        spawn_pty_command(request)
    } else {
        spawn_pipe_command(request).await
    }
}

pub(crate) async fn run_legacy_shell_command(
    command: &str,
    timeout_ms: u64,
) -> Result<LegacyShellCommandResult> {
    let spec = build_shell_command_spec(command, None);
    let mut process = spec.into_tokio_command(None);
    process.stdin(Stdio::null());
    let output = match timeout(Duration::from_millis(timeout_ms), process.output()).await {
        Ok(result) => result.context("failed to execute shell command")?,
        Err(_) => {
            return Ok(LegacyShellCommandResult {
                stdout: String::new(),
                stderr: String::new(),
                status_code: None,
                timed_out: true,
            });
        }
    };

    Ok(LegacyShellCommandResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status_code: output.status.code(),
        timed_out: false,
    })
}

async fn spawn_pipe_command(request: &CommandExecutionRequest) -> Result<SpawnedCommand> {
    let spec = build_shell_command_spec(&request.cmd, request.shell.as_deref());
    debug!(
        command = %request.cmd,
        tty = request.tty,
        workdir = ?request.workdir,
        shell = ?request.shell,
        "spawning pipe command session"
    );

    let mut command = spec.into_tokio_command(request.workdir.as_deref());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn command `{}`", request.cmd))?;
    let stdin = child
        .stdin
        .take()
        .context("failed to capture command stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to capture command stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture command stderr")?;

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<ProcessEvent>();

    tokio::spawn(async move {
        let mut stdin = stdin;
        while let Some(bytes) = input_rx.recv().await {
            if bytes.is_empty() {
                continue;
            }

            if let Err(error) = stdin.write_all(&bytes).await {
                warn!(error = %error, "pipe command stdin write failed");
                break;
            }
            if let Err(error) = stdin.flush().await {
                warn!(error = %error, "pipe command stdin flush failed");
                break;
            }
        }
    });

    spawn_pipe_reader(stdout, event_tx.clone(), "stdout");
    spawn_pipe_reader(stderr, event_tx.clone(), "stderr");
    tokio::spawn(async move {
        let exit_code = match child.wait().await {
            Ok(status) => normalize_exit_code(status.code(), status.success()),
            Err(error) => {
                warn!(error = %error, "pipe command wait failed");
                1
            }
        };
        let _ = event_tx.send(ProcessEvent::Exit(exit_code));
    });

    Ok(SpawnedCommand { input_tx, event_rx })
}

fn spawn_pipe_reader<R>(
    mut reader: R,
    event_tx: mpsc::UnboundedSender<ProcessEvent>,
    stream_name: &'static str,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read_bytes) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read_bytes]).to_string();
                    let _ = event_tx.send(ProcessEvent::Output(chunk));
                }
                Err(error) => {
                    warn!(stream_name, error = %error, "pipe command output read failed");
                    break;
                }
            }
        }
    });
}

fn spawn_pty_command(request: &CommandExecutionRequest) -> Result<SpawnedCommand> {
    let spec = build_shell_command_spec(&request.cmd, request.shell.as_deref());
    debug!(
        command = %request.cmd,
        tty = request.tty,
        workdir = ?request.workdir,
        shell = ?request.shell,
        "spawning pty command session"
    );

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: PTY_DEFAULT_ROWS,
            cols: PTY_DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to allocate PTY for command session")?;
    let builder = spec.into_pty_command_builder(request.workdir.as_deref());
    let mut child = pair
        .slave
        .spawn_command(builder.clone())
        .with_context(|| format!("failed to spawn PTY command `{}`", request.cmd))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("failed to capture PTY writer")?;

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<ProcessEvent>();

    std::thread::spawn(move || {
        handle_pty_writes(&mut input_rx, writer);
    });

    let output_tx = event_tx.clone();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read_bytes) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read_bytes]).to_string();
                    let _ = output_tx.send(ProcessEvent::Output(chunk));
                }
                Err(error) => {
                    warn!(error = %error, "pty command output read failed");
                    break;
                }
            }
        }
    });

    std::thread::spawn(move || {
        let exit_code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(error) => {
                warn!(error = %error, "pty command wait failed");
                1
            }
        };
        let _ = event_tx.send(ProcessEvent::Exit(exit_code));
    });

    Ok(SpawnedCommand { input_tx, event_rx })
}

fn handle_pty_writes(
    input_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    mut writer: Box<dyn Write + Send>,
) {
    while let Some(bytes) = input_rx.blocking_recv() {
        if bytes.is_empty() {
            continue;
        }

        if let Err(error) = writer.write_all(&bytes) {
            warn!(error = %error, "pty command stdin write failed");
            break;
        }
        if let Err(error) = writer.flush() {
            warn!(error = %error, "pty command stdin flush failed");
            break;
        }
    }
}

impl ShellCommandSpec {
    fn into_tokio_command(&self, workdir: Option<&Path>) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(workdir) = workdir {
            command.current_dir(workdir);
        }
        command
    }

    fn into_pty_command_builder(&self, workdir: Option<&Path>) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        if let Some(workdir) = workdir {
            builder.cwd(workdir.as_os_str());
        }
        builder
    }
}

fn build_shell_command_spec(command: &str, shell: Option<&str>) -> ShellCommandSpec {
    #[cfg(windows)]
    {
        let shell = shell.unwrap_or("powershell");
        if is_powershell(shell) {
            let command = normalize_windows_command(command);
            return ShellCommandSpec {
                program: shell.to_string(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    format!("{WINDOWS_UTF8_POWERSHELL_PREFIX} {command}"),
                ],
            };
        }
        if shell.eq_ignore_ascii_case("cmd") || shell.eq_ignore_ascii_case("cmd.exe") {
            return ShellCommandSpec {
                program: shell.to_string(),
                args: vec!["/C".to_string(), command.to_string()],
            };
        }

        ShellCommandSpec {
            program: shell.to_string(),
            args: vec!["-lc".to_string(), command.to_string()],
        }
    }

    #[cfg(not(windows))]
    {
        let shell = shell.unwrap_or("sh");
        ShellCommandSpec {
            program: shell.to_string(),
            args: vec!["-lc".to_string(), command.to_string()],
        }
    }
}

#[cfg(windows)]
fn is_powershell(shell: &str) -> bool {
    let shell = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    shell.eq_ignore_ascii_case("powershell") || shell.eq_ignore_ascii_case("pwsh")
}

#[cfg(windows)]
fn normalize_windows_command(command: &str) -> String {
    match command.trim() {
        "env" | "printenv" => {
            "Get-ChildItem Env: | Sort-Object Name | ForEach-Object { \"{0}={1}\" -f $_.Name, $_.Value }"
                .to_string()
        }
        _ => command.to_string(),
    }
}

fn normalize_exit_code(exit_code: Option<i32>, success: bool) -> i32 {
    exit_code.unwrap_or_else(|| if success { 0 } else { 1 })
}
