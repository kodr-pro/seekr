use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{ExecutionContext, Tool};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

pub struct LspDefinitionTool;

#[async_trait]
impl Tool for LspDefinitionTool {
    fn name(&self) -> &str {
        "lsp_definition"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Find the definition of the symbol at the given position.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "line": { "type": "integer", "description": "1-based line number" },
                        "character": { "type": "integer", "description": "1-based character position" }
                    },
                    "required": ["path", "line", "character"]
                }),
            },
        }
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let line = args["line"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing line"))? as u32;
        let character = args["character"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing character"))? as u32;

        let path = PathBuf::from(path_str);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let (cmd, args_list) = match ext {
            "rs" => ("rust-analyzer", vec![]),
            "py" => ("pyright-langserver", vec!["--stdio"]),
            "go" => ("gopls", vec![]),
            "js" | "ts" => ("typescript-language-server", vec!["--stdio"]),
            _ => return Err(anyhow!("Unsupported file extension: {}", ext)),
        };

        let client_mutex = context.lsp_manager.get_client(ext, cmd, &args_list).await?;
        let mut client = client_mutex.lock().await;

        // Convert 1-based to 0-based for LSP
        let result = client
            .goto_definition(&path, line - 1, character - 1)
            .await?;

        let summary = format!("lsp_definition in {}", crate::tools::short_path(path_str));
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::ActivityStatus::Success,
            thread_id,
            total_threads,
        );

        Ok((serde_json::to_string_pretty(&result)?, summary))
    }
}

pub struct LspReferencesTool;

#[async_trait]
impl Tool for LspReferencesTool {
    fn name(&self) -> &str {
        "lsp_references"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Find all references to the symbol at the given position.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "line": { "type": "integer", "description": "1-based line number" },
                        "character": { "type": "integer", "description": "1-based character position" }
                    },
                    "required": ["path", "line", "character"]
                }),
            },
        }
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let line = args["line"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing line"))? as u32;
        let character = args["character"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing character"))? as u32;

        let path = PathBuf::from(path_str);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let (cmd, args_list) = match ext {
            "rs" => ("rust-analyzer", vec![]),
            "py" => ("pyright-langserver", vec!["--stdio"]),
            "go" => ("gopls", vec![]),
            "js" | "ts" => ("typescript-language-server", vec!["--stdio"]),
            _ => return Err(anyhow!("Unsupported file extension: {}", ext)),
        };

        let client_mutex = context.lsp_manager.get_client(ext, cmd, &args_list).await?;
        let mut client = client_mutex.lock().await;

        let result = client
            .find_references(&path, line - 1, character - 1)
            .await?;

        let summary = format!("lsp_references in {}", crate::tools::short_path(path_str));
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::ActivityStatus::Success,
            thread_id,
            total_threads,
        );

        Ok((serde_json::to_string_pretty(&result)?, summary))
    }
}

pub struct LspHoverTool;

#[async_trait]
impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description:
                    "Get hover information (docs, types) for the symbol at the given position."
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path to the file" },
                        "line": { "type": "integer", "description": "1-based line number" },
                        "character": { "type": "integer", "description": "1-based character position" }
                    },
                    "required": ["path", "line", "character"]
                }),
            },
        }
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing path"))?;
        let line = args["line"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing line"))? as u32;
        let character = args["character"]
            .as_u64()
            .ok_or_else(|| anyhow!("Missing character"))? as u32;

        let path = PathBuf::from(path_str);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let (cmd, args_list) = match ext {
            "rs" => ("rust-analyzer", vec![]),
            "py" => ("pyright-langserver", vec!["--stdio"]),
            "go" => ("gopls", vec![]),
            "js" | "ts" => ("typescript-language-server", vec!["--stdio"]),
            _ => return Err(anyhow!("Unsupported file extension: {}", ext)),
        };

        let client_mutex = context.lsp_manager.get_client(ext, cmd, &args_list).await?;
        let mut client = client_mutex.lock().await;

        let result = client.hover(&path, line - 1, character - 1).await?;

        let summary = format!("lsp_hover in {}", crate::tools::short_path(path_str));
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::ActivityStatus::Success,
            thread_id,
            total_threads,
        );

        Ok((serde_json::to_string_pretty(&result)?, summary))
    }
}
