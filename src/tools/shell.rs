// tools/shell.rs - Shell command execution tool
//
// Runs shell commands via /bin/sh and captures stdout/stderr.
// Includes security sandboxing to prevent dangerous commands.

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncBufReadExt, BufReader};
use std::process::Stdio;
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Tool, truncate, task::TaskManager};
use serde_json::json;

/// Execute a shell command and return the combined output
pub async fn shell_command(args: &serde_json::Value, task_manager: &mut TaskManager) -> Result<(String, String)> {
    let command = args["command"].as_str().ok_or_else(|| anyhow!("Missing command"))?;
    let summary = format!("shell_command {}", truncate(command, 20));
    task_manager.log_activity("shell_command", &summary);

    // Basic security check: prevent dangerous commands
    let dangerous_patterns = ["rm -rf /", "mkfs", "dd if=", ":(){ :|:& };:"];
    for pattern in dangerous_patterns {
        if command.contains(pattern) {
            return Err(anyhow!("Security Error: Command contains a forbidden pattern: '{}'", pattern));
        }
    }

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn command: {}", command))?;

    let stdout = child.stdout.take().context("Failed to open stdout")?;
    let stderr = child.stderr.take().context("Failed to open stderr")?;
    let mut stdin = child.stdin.take().context("Failed to open stdin")?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let mut reader = BufReader::new(stdout);
    let mut stderr_reader = BufReader::new(stderr);

    let result_arc = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let result_clone = result_arc.clone();
    let result_clone_err = result_arc.clone();

    // Spawn stdout reader
    tokio::spawn(async move {
        let mut line = String::new();
        loop {
            let mut byte = [0u8; 1];
            match reader.read_exact(&mut byte).await {
                Ok(_) => {
                    let c = byte[0] as char;
                    line.push(c);
                    {
                        let mut res = result_clone.lock().await;
                        res.push(c);
                    }
                    
                    // Check for common prompt patterns
                    let prompt_patterns = ["[sudo] password", "(y/n)", "[Y/n]", "Password:", "confirm", "Enter something:"];
                    for pattern in prompt_patterns {
                        if line.to_lowercase().contains(&pattern.to_lowercase()) {
                            tx.send(line.clone()).ok();
                            line.clear();
                        }
                    }
                    
                    if c == '\n' {
                        line.clear();
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Spawn stderr reader
    tokio::spawn(async move {
        let mut line = String::new();
        while let Ok(n) = stderr_reader.read_line(&mut line).await {
            if n == 0 { break; }
            let mut res = result_clone_err.lock().await;
            res.push_str("[stderr] ");
            res.push_str(&line);
            line.clear();
        }
    });

    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    task_manager.set_input_tx(input_tx);

    loop {
        tokio::select! {
            Some(prompt) = rx.recv() => {
                if let Some(ref event_tx) = task_manager.event_tx {
                    event_tx.send(crate::agent::AgentEvent::CliInputRequest { prompt }).ok();
                }
            }
            Some(input) = input_rx.recv() => {
                let _ = stdin.write_all(input.as_bytes()).await;
                let _ = stdin.write_all(b"\n").await;
                let _ = stdin.flush().await;
            }
            status = child.wait() => {
                let status = status?;
                let mut final_res = result_arc.lock().await.clone();
                if final_res.is_empty() {
                    final_res = format!("Command completed with exit code: {}", status.code().unwrap_or(-1));
                } else {
                    final_res.push_str(&format!(
                        "\n[exit code: {}]",
                        status.code().unwrap_or(-1)
                    ));
                }
                
                if final_res.len() > 16_000 {
                    final_res.truncate(16_000);
                    final_res.push_str("\n... [output truncated]");
                }
                return Ok((final_res, summary));
            }
        }
    }
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

    async fn execute(&self, args: &serde_json::Value, task_manager: &mut TaskManager) -> Result<(String, String)> {
        shell_command(args, task_manager).await
    }
}
