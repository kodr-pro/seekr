use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{short_path, task::TaskManager, Tool};
use crate::errors::ToolError;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};

fn resolve_and_verify_path(requested: &str, task_manager: &TaskManager) -> Result<PathBuf> {
    // 1. Determine working directory from config
    let working_dir = if let Some(config) = &task_manager.config {
        PathBuf::from(&config.agent.working_directory)
    } else {
        PathBuf::from(".")
    };

    // 2. Resolve to absolute path
    let base_absolute =
        std::fs::canonicalize(&working_dir).unwrap_or_else(|_| current_dir_or_root());

    let requested_path = Path::new(requested);
    let absolute_requested = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        base_absolute.join(requested_path)
    };

    // 3. Clean the path to handle ".." and "."
    let cleaned_requested = clean_path(&absolute_requested);

    // 4. Verify it starts with the base_absolute directory
    if !cleaned_requested.starts_with(&base_absolute)
        && !working_dir.as_os_str().is_empty()
        && working_dir.to_string_lossy() != "."
        && working_dir.to_string_lossy() != "./"
    {
        return Err(ToolError::SecurityError(format!(
            "Access denied. Path '{}' is outside the permitted working directory.",
            requested
        )).into());
    }

    Ok(cleaned_requested)
} // resolve_and_verify_path

fn current_dir_or_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            _ => out.push(c),
        }
    }
    out
} // clean_path

pub async fn read_file(path: &PathBuf) -> Result<String> {
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file: {}", path.display()))
} // read_file

pub async fn write_file(path: &PathBuf, content: &str) -> Result<String> {
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
    Ok(format!(
        "Successfully wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
} // write_file

pub async fn edit_file(path: &PathBuf, old_string: &str, new_string: &str) -> Result<String> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file for editing: {}", path.display()))?;

    if !contents.contains(old_string) {
        return Err(ToolError::EditFailed(format!(
            "The string to replace was not found in {}. Make sure the old_string matches exactly.",
            path.display()
        )).into());
    }

    let new_contents = contents.replacen(old_string, new_string, 1);
    tokio::fs::write(path, &new_contents)
        .await
        .with_context(|| format!("Failed to write edited file: {}", path.display()))?;

    Ok(format!("Successfully edited {}", path.display()))
} // edit_file

pub async fn list_directory(path: &PathBuf) -> Result<String> {
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
} // list_directory

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Read the contents of a file.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Path to the file to read" } },
                    "required": ["path"]
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
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let summary = format!("read_file {}", short_path(path));
        task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let resolved_path = resolve_and_verify_path(path, task_manager)?;
        let result = read_file(&resolved_path).await?;

        Ok((result, summary))
    } // execute
} // impl ReadFileTool

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Write content to a file.".to_string(),
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
    } // definition
    async fn execute(
        &self,
        args: &serde_json::Value,
        task_manager: &TaskManager,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing content"))?;
        let summary = format!("write_file {}", short_path(path));
        task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let resolved_path = resolve_and_verify_path(path, task_manager)?;
        let result = write_file(&resolved_path, content).await?;

        Ok((result, summary))
    } // execute
} // impl WriteFileTool

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Edit a file by replacing a specific string with a new string."
                    .to_string(),
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
    } // definition
    async fn execute(
        &self,
        args: &serde_json::Value,
        task_manager: &TaskManager,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let old = args["old_string"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing old_string"))?;
        let new = args["new_string"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing new_string"))?;
        let summary = format!("edit_file {}", short_path(path));
        task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let resolved_path = resolve_and_verify_path(path, task_manager)?;
        let result = edit_file(&resolved_path, old, new).await?;

        Ok((result, summary))
    } // execute
} // impl EditFileTool

pub struct ListDirectoryTool;

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "List files and directories at the given path.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Directory path to list" } },
                    "required": []
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
        let path = args["path"].as_str().unwrap_or(".");
        let summary = format!("list_directory {}", short_path(path));
        task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let resolved_path = resolve_and_verify_path(path, task_manager)?;
        let result = list_directory(&resolved_path).await?;

        Ok((result, summary))
    } // execute
} // impl ListDirectoryTool
