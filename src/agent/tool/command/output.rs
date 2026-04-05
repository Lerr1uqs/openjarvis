//! Formatting helpers for command session summaries and task listings.

use super::session::{CommandExecutionResult, CommandTaskSnapshot};
use chrono::SecondsFormat;

const DEFAULT_MAX_OUTPUT_TOKENS: usize = 4_000;
const EXITED_DRAINED_OUTPUT_PLACEHOLDER: &str = "NULL (\u{5f53}\u{524d}\u{7a0b}\u{5e8f}\u{5df2}\u{7ed3}\u{675f}\u{ff0c}\u{7f13}\u{51b2}\u{533a}\u{8bfb}\u{53d6}\u{5b8c}\u{6bd5})";

pub(crate) fn approximate_token_count(content: &str) -> usize {
    if content.is_empty() {
        return 0;
    }

    content.chars().count().div_ceil(4)
}

pub(crate) fn truncate_output(content: &str, max_output_tokens: Option<usize>) -> (String, usize) {
    let original_token_count = approximate_token_count(content);
    let max_output_tokens = max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
    if max_output_tokens == 0 {
        return (String::new(), original_token_count);
    }

    let max_chars = max_output_tokens.saturating_mul(4);
    let mut truncated = String::new();
    for (index, ch) in content.chars().enumerate() {
        if index >= max_chars {
            break;
        }
        truncated.push(ch);
    }
    (truncated, original_token_count)
}

pub(crate) fn format_command_summary(result: &CommandExecutionResult) -> String {
    let status_line = if result.running {
        format!(
            "Process running with session ID {}",
            result
                .session_id
                .as_deref()
                .unwrap_or("unknown-command-session")
        )
    } else {
        format!(
            "Process exited with code {}",
            result.exit_code.unwrap_or_default()
        )
    };
    let output_section = format_output_section(result);

    format!(
        "Command: {}\nChunk ID: {}\nWall time: {:.3}s\n{}\nOriginal token count: {}\n{}",
        result.command,
        result.chunk_id,
        result.wall_time_seconds,
        status_line,
        result.original_token_count,
        output_section
    )
}

fn format_output_section(result: &CommandExecutionResult) -> String {
    // Only surface the drained placeholder when the underlying chunk is truly empty,
    // instead of mislabeling output that was truncated away by `max_output_tokens = 0`.
    if result.output.is_empty() && result.original_token_count == 0 && !result.running {
        return format!("Output: {EXITED_DRAINED_OUTPUT_PLACEHOLDER}");
    }

    format!("Output:\n{}", result.output)
}

pub(crate) fn format_task_listing(tasks: &[CommandTaskSnapshot]) -> String {
    if tasks.is_empty() {
        return "No unread command task output.".to_string();
    }

    let body = tasks
        .iter()
        .map(|task| {
            format!(
                "- {} | exit_code={} | {} | {}",
                task.session_id,
                task.exit_code
                    .map(|exit_code| exit_code.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                task.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                task.command
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("Unread command task output:\n{body}")
}
