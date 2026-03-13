// tools/file_edit.rs - File reading, writing, and editing tools
//
// Provides read_file, write_file, edit_file, and list_directory tool implementations.
// All operations are relative to the configured working directory.

use anyhow::{Context, Result, anyhow};
use std::path::Path;
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Tool, task::TaskManager, short_path};
use serde_json::json;

/// Read the contents of a file at the given path
pub async fn read_file(path: &str) -> Result<String> {
    let path = Path::new(path);
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file: {}", path.display()))
}

/// Write content to a file, creating it if it doesn't exist
pub async fn write_file(path: &str, content: &str) -> Result<String> {
    let path = Path::new(path);
    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("Failed to write file: {}", path.display()))?;
    Ok(format!("Successfully wrote {} bytes to {}", content.len(), path.display()))
}

/// Edit a file by replacing an exact string match with a new string
pub async fn edit_file(path: &str, old_string: &str, new_string: &str) -> Result<String> {
    let path = Path::new(path);
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file for editing: {}", path.display()))?;

    if !contents.contains(old_string) {
        anyhow::bail!(
            "The string to replace was not found in {}. Make sure the old_string matches exactly.",
            path.display()
        );
    }

    let new_contents = contents.replacen(old_string, new_string, 1);
    tokio::fs::write(path, &new_contents)
        .await
        .with_context(|| format!("Failed to write edited file: {}", path.display()))?;

    Ok(format!("Successfully edited {}", path.display()))
}

/// List files and directories at the given path
pub async fn list_directory(path: &str) -> Result<String> {
    let dir_path = if path.is_empty() { "." } else { path };
    let path = Path::new(dir_path);

    let mut entries = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("Failed to list directory: {}", path.display()))?;

    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let name = entry.file_name().to_string_lossy().to_string();
        let indicator = if file_type.is_dir() { "/" } else { "" };
        items.push(format!("{}{}", name, indicator));
    }

    items.sort();
    if items.is_empty() {
        Ok(format!("Directory {} is empty", path.display()))
    } else {
        Ok(items.join("\n"))
    }
}

// --- Tools ---

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Read the contents of a file. Returns the file content as a string.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file to read" }
                    },
                    "required": ["path"]
                }),
            },
        }
    }
    async fn execute(&self, args: &serde_json::Value, _task_manager: &mut TaskManager) -> Result<(String, String)> {
        let path = args["path"].as_str().ok_or_else(|| anyhow!("Missing path"))?;
        let summary = format!("read_file {}", short_path(path));
        let result = read_file(path).await?;
        Ok((result, summary))
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to write the file" },
                        "content": { "type": "string", "description": "Content to write" }
                    },
                    "required": ["path", "content"]
                }),
            },
        }
    }
    async fn execute(&self, args: &serde_json::Value, _task_manager: &mut TaskManager) -> Result<(String, String)> {
        let path = args["path"].as_str().ok_or_else(|| anyhow!("Missing path"))?;
        let content = args["content"].as_str().ok_or_else(|| anyhow!("Missing content"))?;
        let summary = format!("write_file {}", short_path(path));
        let result = write_file(path, content).await?;
        Ok((result, summary))
    }
}

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str { "edit_file" }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Edit a file by replacing a specific string with a new string. Use read_file first to see current content.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file to edit" },
                        "old_string": { "type": "string", "description": "Exact string to find and replace" },
                        "new_string": { "type": "string", "description": "Replacement string" }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            },
        }
    }
    async fn execute(&self, args: &serde_json::Value, _task_manager: &mut TaskManager) -> Result<(String, String)> {
        let path = args["path"].as_str().ok_or_else(|| anyhow!("Missing path"))?;
        let old = args["old_string"].as_str().ok_or_else(|| anyhow!("Missing old_string"))?;
        let new = args["new_string"].as_str().ok_or_else(|| anyhow!("Missing new_string"))?;
        let summary = format!("edit_file {}", short_path(path));
        let result = edit_file(path, old, new).await?;
        Ok((result, summary))
    }
}

pub struct ListDirectoryTool;

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str { "list_directory" }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "List files and directories at the given path.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path to list (default: current directory)" }
                    },
                    "required": []
                }),
            },
        }
    }
    async fn execute(&self, args: &serde_json::Value, _task_manager: &mut TaskManager) -> Result<(String, String)> {
        let path = args["path"].as_str().unwrap_or(".");
        let summary = format!("list_directory {}", short_path(path));
        let result = list_directory(path).await?;
        Ok((result, summary))
    }
}
