use anyhow::{Context, Result, anyhow};
use tokio::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::process::Stdio;
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Tool, truncate, task::TaskManager};
use serde_json::json;

fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    
    while let Some(c) = iter.next() {
        if c == '\x1b' {
            if let Some('[') = iter.peek() {
                iter.next();
                while let Some(&next) = iter.peek() {
                    iter.next();
                    if (next >= 'A' && next <= 'Z') || (next >= 'a' && next <= 'z') || next == '@' {
                        break;
                    }
                }
                continue;
            } else if let Some(']') = iter.peek() {
                iter.next();
                while let Some(&next) = iter.peek() {
                    iter.next();
                    if next == '\x07' || next == '\x5c' {
                        break;
                    }
                }
                continue;
            }
        }
        result.push(c);
    }
    result
} // strip_ansi_codes

fn detect_prompt(line: &str) -> Option<String> {
    let stripped = strip_ansi_codes(line);
    let prompt_patterns = ["[sudo] password", "(y/n)", "[Y/n]", "Password:", "confirm", "Enter something:"];
    for pattern in prompt_patterns {
        if stripped.to_lowercase().contains(&pattern.to_lowercase()) {
            return Some(stripped);
        }
    }
    None
} // detect_prompt

pub async fn shell_command(
    args: &serde_json::Value, 
    task_manager: &TaskManager,
    thread_id: Option<usize>,
    total_threads: Option<usize>,
) -> Result<(String, String)> {
    let command = args["command"].as_str().ok_or_else(|| anyhow!("Missing command"))?;
    let background = args["background"].as_bool().unwrap_or(false);
    
    let summary = format!("shell_command {}", truncate(command, 20));
    task_manager.log_activity("shell_command", &summary, crate::tools::task::ActivityStatus::Starting, thread_id, total_threads);

    let dangerous_patterns = [
        "rm -rf /", "mkfs", "dd if=", ":(){ :|:& };:", 
        "> /dev/sda", "> /dev/nvme", "chmod -R 777 /", "chown -R"
    ];
    for pattern in dangerous_patterns {
        if command.contains(pattern) {
            return Err(anyhow!("Security Error: Command contains a forbidden pattern: '{}'", pattern));
        }
    }

    if background {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to spawn background command: {}", command))?;
        
        let res = format!("Command started in background: {}", command);
        return Ok((res, format!("bg: {}", truncate(command, 15))));
    }

    let timeout_duration = std::time::Duration::from_secs(300);

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

    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let context_arc = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let context_clone_out = context_arc.clone();
    let context_clone_err = context_arc.clone();

    let tx_out = prompt_tx.clone();
    let tx_err = prompt_tx.clone();

    let mut reader = tokio::io::BufReader::new(stdout);
    let mut stderr_reader = tokio::io::BufReader::new(stderr);

    let result_arc = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let result_clone = result_arc.clone();
    let result_clone_err = result_arc.clone();

    let stdout_handle = tokio::spawn(async move {
        let mut line = String::new();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    let (valid_up_to, more_needed) = match std::str::from_utf8(&buffer) {
                        Ok(s) => (s.len(), false),
                        Err(e) => (e.valid_up_to(), e.error_len().is_none()),
                    };
                    if valid_up_to > 0 {
                        let s = String::from_utf8_lossy(&buffer[..valid_up_to]).into_owned();
                        for c in s.chars() {
                            line.push(c);
                            {
                                let mut res = result_clone.lock().await;
                                res.push(c);
                            }
                            if c == '\n' {
                                let trimmed = line.trim().to_string();
                                if !trimmed.is_empty() {
                                    let mut ctx = context_clone_out.lock().await;
                                    ctx.push(trimmed.clone());
                                    if ctx.len() > 5 { ctx.remove(0); }
                                }
                                if let Some(prompt) = detect_prompt(&line) {
                                    tx_out.send(prompt).ok();
                                }
                                line.clear();
                            }
                        }
                        if !line.is_empty() {
                            if let Some(prompt) = detect_prompt(&line) {
                                tx_out.send(prompt).ok();
                            }
                        }
                        buffer.drain(..valid_up_to);
                    }
                    if !more_needed && !buffer.is_empty() {
                        buffer.remove(0);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stderr_handle = tokio::spawn(async move {
        let mut line = String::new();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stderr_reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    let (valid_up_to, more_needed) = match std::str::from_utf8(&buffer) {
                        Ok(s) => (s.len(), false),
                        Err(e) => (e.valid_up_to(), e.error_len().is_none()),
                    };
                    if valid_up_to > 0 {
                        let s = String::from_utf8_lossy(&buffer[..valid_up_to]).into_owned();
                        for c in s.chars() {
                            line.push(c);
                            {
                                let mut res = result_clone_err.lock().await;
                                if line.len() == 1 { res.push_str("[stderr] "); }
                                res.push(c);
                            }
                            if c == '\n' {
                                let trimmed = line.trim().to_string();
                                if !trimmed.is_empty() {
                                    let mut ctx = context_clone_err.lock().await;
                                    ctx.push(trimmed.clone());
                                    if ctx.len() > 5 { ctx.remove(0); }
                                }
                                if let Some(prompt) = detect_prompt(&line) {
                                    tx_err.send(prompt).ok();
                                }
                                line.clear();
                            }
                        }
                        if !line.is_empty() {
                            if let Some(prompt) = detect_prompt(&line) {
                                tx_err.send(prompt).ok();
                            }
                        }
                        buffer.drain(..valid_up_to);
                    }
                    if !more_needed && !buffer.is_empty() {
                        buffer.remove(0);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    loop {
        tokio::select! {
            Some(_prompt) = prompt_rx.recv() => {
                if let Some(ref event_tx) = task_manager.event_tx {
                    let ctx_lines = context_arc.lock().await.join("\n");
                    event_tx.send(crate::agent::AgentEvent::ShellInputNeeded {
                        context: ctx_lines,
                        input_tx: input_tx.clone(),
                    }).ok();
                }
            }
            Some(input) = input_rx.recv() => {
                let _ = stdin.write_all(input.as_bytes()).await;
                if !input.ends_with('\n') {
                    let _ = stdin.write_all(b"\n").await;
                }
                let _ = stdin.flush().await;
            }
            status = tokio::time::timeout(timeout_duration, child.wait()) => {
                let status = match status {
                    Ok(s) => s?,
                    Err(_) => {
                        let _ = child.kill().await;
                        return Err(anyhow!("Command timed out after {} seconds", timeout_duration.as_secs()));
                    }
                };

                let _ = stdout_handle.await;
                let _ = stderr_handle.await;

                let mut final_res = result_arc.lock().await.clone();
                if final_res.is_empty() {
                    final_res = format!("Command completed with exit code: {}", status.code().unwrap_or(-1));
                } else {
                    final_res.push_str(&format!("\n[exit code: {}]", status.code().unwrap_or(-1)));
                }
                if final_res.len() > 16_000 {
                    final_res.truncate(16_000);
                    final_res.push_str("\n... [output truncated]");
                }
                return Ok((final_res, summary));
            }
        }
    }
} // shell_command

pub struct ShellCommandTool;

#[async_trait]
impl Tool for ShellCommandTool {
    fn name(&self) -> &str {
        "shell_command"
    } // name

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Execute a shell command and return stdout/stderr.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to execute" },
                        "background": { "type": "boolean", "description": "Whether to run the command in the background" }
                    },
                    "required": ["command"]
                }),
            },
        }
    } // definition

    async fn execute(
        &self, 
        args: &serde_json::Value, 
        task_manager: &TaskManager,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        shell_command(args, task_manager, thread_id, total_threads).await
    } // execute
} // impl ShellCommandTool

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes() {
        let input = "\x1b[2K[sudo] password for user: ";
        let expected = "[sudo] password for user: ";
        assert_eq!(strip_ansi_codes(input), expected);

        let input2 = "\x1b[31mError:\x1b[0m critical failure";
        let expected2 = "Error: critical failure";
        assert_eq!(strip_ansi_codes(input2), expected2);
    } // test_strip_ansi_codes

    #[test]
    fn test_detect_prompt() {
        assert!(detect_prompt("[sudo] password for user: ").is_some());
        assert!(detect_prompt("\x1b[2KPassword:").is_some());
        assert!(detect_prompt("regular output").is_none());
        assert!(detect_prompt("confirm execution? (y/n)").is_some());
    } // test_detect_prompt
} // tests
