// tools/shell.rs - Shell command execution tool
//
// Runs shell commands via /bin/sh and captures stdout/stderr.
// Used for compilation, running tests, git operations, etc.

use anyhow::{Context, Result};
use tokio::process::Command;

/// Execute a shell command and return the combined output
pub async fn shell_command(command: &str) -> Result<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await
        .with_context(|| format!("Failed to execute command: {}", command))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let mut result = String::new();

    if !stdout.is_empty() {
        result.push_str(&stdout);
    }

    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr);
    }

    if result.is_empty() {
        result = format!("Command completed with exit code: {}", output.status.code().unwrap_or(-1));
    } else {
        result.push_str(&format!(
            "\n[exit code: {}]",
            output.status.code().unwrap_or(-1)
        ));
    }

    // Truncate very long output to avoid token overflow
    if result.len() > 16_000 {
        result.truncate(16_000);
        result.push_str("\n... [output truncated]");
    }

    Ok(result)
}
