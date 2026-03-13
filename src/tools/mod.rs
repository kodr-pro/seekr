// tools/mod.rs - Tool registry, definitions, and dispatch
//
// Centralizes all tool definitions (for the API) and dispatches
// tool calls to the appropriate implementation.

pub mod file_edit;
pub mod shell;
pub mod task;
pub mod web;

use crate::api::types::{FunctionDefinition, ToolDefinition};
use serde_json::json;

/// An entry in the activity log shown in the task panel
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    #[allow(dead_code)]
    pub tool_name: String,
    pub summary: String,
}

/// Build all tool definitions for the DeepSeek API request
pub fn all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read the contents of a file. Returns the file content as a string."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "write_file".to_string(),
                description:
                    "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to write the file"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write"
                        }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "edit_file".to_string(),
                description:
                    "Edit a file by replacing a specific string with a new string. Use read_file first to see current content."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to edit"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "Exact string to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement string"
                        }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "list_directory".to_string(),
                description: "List files and directories at the given path.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path to list (default: current directory)"
                        }
                    },
                    "required": []
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "shell_command".to_string(),
                description:
                    "Execute a shell command and return stdout/stderr. Use for compilation, running tests, git operations, etc."
                        .to_string(),
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
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_fetch".to_string(),
                description:
                    "Fetch a web page and return its text content (HTML stripped). Use to gather information from the web."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL to fetch"
                        },
                        "selector": {
                            "type": "string",
                            "description": "Optional CSS selector to extract specific content"
                        }
                    },
                    "required": ["url"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_search".to_string(),
                description:
                    "Search the web using DuckDuckGo and return results with titles, URLs, and snippets."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        }
                    },
                    "required": ["query"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "create_task".to_string(),
                description:
                    "Create a task to track progress on a multi-step operation. Tasks appear in the task panel."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Short task title"
                        },
                        "status": {
                            "type": "string",
                            "description": "Task status: pending, in_progress, completed, failed"
                        }
                    },
                    "required": ["title"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "update_task".to_string(),
                description: "Update the status of an existing task.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "Task ID (1-based index)"
                        },
                        "status": {
                            "type": "string",
                            "description": "New status: pending, in_progress, completed, failed"
                        }
                    },
                    "required": ["task_id", "status"]
                }),
            },
        },
    ]
}

/// Execute a tool call by name and return the result string.
/// Task-related tools require a mutable reference to the TaskManager.
pub async fn execute_tool(
    name: &str,
    arguments: &str,
    task_manager: &mut task::TaskManager,
) -> (String, ActivityEntry) {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(json!({}));

    let (result, summary) = match name {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let summary = format!("read_file {}", short_path(path));
            match file_edit::read_file(path).await {
                Ok(content) => (content, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "write_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");
            let summary = format!("write_file {}", short_path(path));
            match file_edit::write_file(path, content).await {
                Ok(msg) => (msg, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "edit_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let old = args["old_string"].as_str().unwrap_or("");
            let new = args["new_string"].as_str().unwrap_or("");
            let summary = format!("edit_file {}", short_path(path));
            match file_edit::edit_file(path, old, new).await {
                Ok(msg) => (msg, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "list_directory" => {
            let path = args["path"].as_str().unwrap_or(".");
            let summary = format!("list_directory {}", short_path(path));
            match file_edit::list_directory(path).await {
                Ok(listing) => (listing, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "shell_command" => {
            let command = args["command"].as_str().unwrap_or("");
            let summary = format!(
                "shell {}",
                if command.len() > 30 {
                    format!("{}...", &command[..30])
                } else {
                    command.to_string()
                }
            );
            match shell::shell_command(command).await {
                Ok(output) => (output, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "web_fetch" => {
            let url = args["url"].as_str().unwrap_or("");
            let selector = args["selector"].as_str();
            let summary = format!("web_fetch {}", short_url(url));
            match web::web_fetch(url, selector).await {
                Ok(content) => (content, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "web_search" => {
            let query = args["query"].as_str().unwrap_or("");
            let summary = format!("web_search \"{}\"", truncate(query, 20));
            match web::web_search(query).await {
                Ok(results) => (results, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        "create_task" => {
            let title = args["title"].as_str().unwrap_or("Untitled");
            let status = args["status"].as_str();
            let id = task_manager.create_task(title, status);
            let summary = format!("create_task #{}", id);
            (format!("Created task #{}: {}", id, title), summary)
        }
        "update_task" => {
            let task_id = args["task_id"].as_u64().unwrap_or(0) as usize;
            let status = args["status"].as_str().unwrap_or("pending");
            let summary = format!("update_task #{}", task_id);
            match task_manager.update_task(task_id, status) {
                Ok(msg) => (msg, summary),
                Err(e) => (format!("Error: {e}"), summary),
            }
        }
        _ => {
            let summary = format!("unknown: {}", name);
            (format!("Unknown tool: {}", name), summary)
        }
    };

    let activity = ActivityEntry {
        tool_name: name.to_string(),
        summary,
    };

    (result, activity)
}

/// Shorten a file path for activity display
fn short_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Shorten a URL for activity display
fn short_url(url: &str) -> String {
    if url.len() > 40 {
        format!("{}...", &url[..40])
    } else {
        url.to_string()
    }
}

/// Truncate a string for display
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    }
}
