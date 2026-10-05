use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

/// Result of a sandboxed command execution.
#[derive(Clone, Debug)]
pub struct CommandOutcome {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutcome {
    /// Combined output suitable for feeding back to a model (stderr first).
    pub fn combined(&self) -> String {
        let mut out = String::new();
        if !self.stdout.is_empty() {
            out.push_str(&format!(
                "--- stdout ---\n{}\n",
                truncate(&self.stdout)
            ));
        }
        if !self.stderr.is_empty() {
            out.push_str(&format!(
                "--- stderr ---\n{}\n",
                truncate(&self.stderr)
            ));
        }
        if self.timed_out {
            out.push_str("--- killed: command exceeded its time budget ---\n");
        }
        if out.is_empty() {
            out.push_str("(no output)");
        }
        out
    }
}

const MAX_OUTPUT_CHARS: usize = 16_000;

fn truncate(s: &str) -> &str {
    if s.len() <= MAX_OUTPUT_CHARS {
        s
    } else {
        let mut end = MAX_OUTPUT_CHARS;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Rejects commands matching the configured blocklist (substring match,
/// matching the interactive agent's policy).
pub fn blocklist_violation(
    command: &str,
    blocklist: &[String],
) -> Option<String> {
    blocklist
        .iter()
        .find(|pattern| command.contains(pattern.as_str()))
        .cloned()
}

/// True for commands that only inspect the workspace and cannot mutate it.
/// These bypass the Jev scope gate (same trust level as read_file).
pub fn is_readonly_command(command: &str) -> bool {
    let trimmed = command.trim();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let readonly_binaries = [
        "pwd", "ls", "cat", "head", "tail", "grep", "find", "wc", "test",
        "echo", "which", "true", "false", "stat", "file", "du", "sort", "uniq",
        "diff", "rg",
    ];
    if readonly_binaries.contains(&first) {
        return true;
    }
    if first == "git" {
        let second = trimmed.split_whitespace().nth(1).unwrap_or("");
        return matches!(
            second,
            "status" | "diff" | "log" | "show" | "ls-files" | "branch"
        );
    }
    false
}

/// Runs `sh -c <command>` pinned to `cwd` with a hard wall-clock budget.
/// Output is captured (never inherited), truncated, and ANSI-stripped.
pub async fn run_command(
    cwd: &Path,
    command: &str,
    timeout_secs: u64,
    blocklist: &[String],
) -> std::io::Result<CommandOutcome> {
    if let Some(pattern) = blocklist_violation(command, blocklist) {
        return Ok(CommandOutcome {
            success: false,
            exit_code: None,
            timed_out: false,
            stdout: String::new(),
            stderr: format!("blocked by sandbox policy: matched '{pattern}'"),
        });
    }

    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let timeout = std::time::Duration::from_secs(timeout_secs.max(1));
    let output =
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Ok(CommandOutcome {
                    success: false,
                    exit_code: None,
                    timed_out: true,
                    stdout: String::new(),
                    stderr: "command timed out".to_string(),
                });
            }
        };

    Ok(CommandOutcome {
        success: output.status.success(),
        exit_code: output.status.code(),
        timed_out: false,
        stdout: strip_ansi(&String::from_utf8_lossy(&output.stdout)),
        stderr: strip_ansi(&String::from_utf8_lossy(&output.stderr)),
    })
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_command_in_cwd_and_captures() {
        let tmp = tempfile::tempdir().unwrap();
        let out =
            run_command(tmp.path(), "echo hello && echo err >&2", 10, &[])
                .await
                .unwrap();
        assert!(out.success);
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn timeout_kills_runaway_command() {
        let tmp = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        let out = run_command(tmp.path(), "sleep 30", 1, &[]).await.unwrap();
        assert!(out.timed_out);
        assert!(!out.success);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn blocklist_denies_destructive_patterns() {
        let blocklist = vec!["rm -rf /".to_string(), "mkfs".to_string()];
        let out = run_command(Path::new("/"), "rm -rf /", 5, &blocklist)
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.stderr.contains("blocked by sandbox policy"));
    }

    #[test]
    fn ansi_codes_are_stripped() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn combined_truncates_huge_output() {
        let out = CommandOutcome {
            success: true,
            exit_code: Some(0),
            timed_out: false,
            stdout: "x".repeat(50_000),
            stderr: String::new(),
        };
        assert!(out.combined().len() < 20_000);
    }
}
