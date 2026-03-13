// tools/shell.rs - Shell command execution tool
//
// Runs shell commands via /bin/sh and captures stdout/stderr.
// Includes security sandboxing to prevent dangerous commands.

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Tool, task::TaskManager};
use serde_json::json;

/// Execute a shell command and return the combined output
pub async fn shell_command(command: &str) -> Result<String> {
    // Basic security check: prevent dangerous commands
    let dangerous_patterns = ["rm -rf /", "mkfs", "dd if=", ":(){ :|:& };:"];
    for pattern in dangerous_patterns {
        if command.contains(pattern) {
            return Err(anyhow!("Security Error: Command contains a forbidden pattern: '{}'", pattern));
        }
    }

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

// --- Tools ---

pub struct ShellCommandTool;

#[async_trait]
impl Tool for ShellCommandTool {
    fn name(&self) -> &str {
        "shell_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Execute a shell command and return stdout/stderr. Use for compilation, running tests, git operations, etc.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        }
                    },
                    "required": ["command"]
                }),
            },
        }
    }

    async fn execute(&self, args: &serde_json::Value, _task_manager: &mut TaskManager) -> Result<(String, String)> {
        let command = args["command"].as_str().ok_or_else(|| anyhow!("Missing command"))?;
        let summary = format!(
            "shell {}",
            if command.len() > 30 {
                format!("{}...", &command[..30])
            } else {
                command.to_string()
            }
        );
        let result = shell_command(command).await?;
        Ok((result, summary))
    }
}
