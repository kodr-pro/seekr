// tools/file_edit.rs - File reading, writing, and editing tools
//
// Provides read_file, write_file, edit_file, and list_directory tool implementations.
// All operations are relative to the configured working directory.

use anyhow::{Context, Result};
use std::path::Path;

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
