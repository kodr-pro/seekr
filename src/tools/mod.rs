// tools/mod.rs - Tool registry and execution
//
// Defines the Tool trait and manages a registry of available tools.
// Each tool is responsible for its own definition and execution logic.

pub mod file_edit;
pub mod shell;
pub mod task;
pub mod web;

use async_trait::async_trait;
use crate::api::types::ToolDefinition;
use crate::tools::task::TaskManager;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

/// An entry in the activity log shown in the task panel
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    #[allow(dead_code)]
    pub tool_name: String,
    pub summary: String,
}

/// The base trait for all agent tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's name
    fn name(&self) -> &str;
    
    /// Returns the JSON definition for the API
    fn definition(&self) -> ToolDefinition;
    
    /// Executes the tool with the given arguments
    async fn execute(
        &self, 
        args: &serde_json::Value, 
        task_manager: &mut TaskManager
    ) -> Result<(String, String)>; // (ResultString, Summary)
}

/// Registry containing all available tools
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };
        
        // Register all core tools
        registry.register(Arc::new(file_edit::ReadFileTool));
        registry.register(Arc::new(file_edit::WriteFileTool));
        registry.register(Arc::new(file_edit::EditFileTool));
        registry.register(Arc::new(file_edit::ListDirectoryTool));
        registry.register(Arc::new(shell::ShellCommandTool));
        registry.register(Arc::new(web::WebFetchTool));
        registry.register(Arc::new(web::WebSearchTool));
        registry.register(Arc::new(task::CreateTaskTool));
        registry.register(Arc::new(task::UpdateTaskTool));
        
        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn all_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }
}

/// Legacy wrapper for the agent loop to use the new registry system
pub async fn execute_tool(
    name: &str,
    arguments: &str,
    task_manager: &mut TaskManager,
) -> (String, ActivityEntry) {
    let registry = ToolRegistry::new();
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::json!({}));

    if let Some(tool) = registry.get(name) {
        match tool.execute(&args, task_manager).await {
            Ok((result, summary)) => (
                result,
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary,
                },
            ),
            Err(e) => (
                format!("Error: {}", e),
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary: format!("{} failed", name),
                },
            ),
        }
    } else {
        (
            format!("Unknown tool: {}", name),
            ActivityEntry {
                tool_name: name.to_string(),
                summary: format!("unknown: {}", name),
            },
        )
    }
}

/// Build all tool definitions for the DeepSeek API request
pub fn all_tool_definitions() -> Vec<ToolDefinition> {
    ToolRegistry::new().all_definitions()
}

// Utility functions for tool implementations

pub fn short_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    }
}
