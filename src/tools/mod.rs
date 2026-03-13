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

pub use crate::tools::task::{ActivityEntry, ActivityStatus};

/// Metadata for a tool or skill
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    pub version: String,
}

/// The base trait for all agent tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's name (must be unique)
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

/// A collection of related tools
pub trait Skill: Send + Sync {
    /// Returns metadata for this skill
    fn metadata(&self) -> Metadata;
    
    /// Returns all tools provided by this skill
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

/// Registry containing all available skills and their tools
pub struct SkillRegistry {
    skills: Vec<Arc<dyn Skill>>,
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl SkillRegistry {
    pub fn new(working_dir: Option<&str>) -> Self {
        let mut registry = Self {
            skills: Vec::new(),
            tools: HashMap::new(),
        };
        
        // Register core skills
        registry.register_skill(Arc::new(CoreSkill));

        // Load global skills
        if let Some(config_dir) = dirs::config_dir() {
            let global_skills_path = config_dir.join("seekr").join("skills");
            if global_skills_path.exists() {
                registry.load_skills_from_dir(&global_skills_path);
            }
        }

        // Load local skills
        if let Some(wd) = working_dir {
            let expanded_wd = shellexpand::tilde(wd);
            let local_skills_path = std::path::Path::new(expanded_wd.as_ref()).join(".seekr").join("skills");
            if local_skills_path.exists() {
                registry.load_skills_from_dir(&local_skills_path);
            }
        }
        
        registry
    }

    fn load_skills_from_dir(&mut self, path: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let skill_path = entry.path();
                if skill_path.is_dir() {
                    let config_path = skill_path.join("skill.json");
                    if config_path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&config_path) {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                                let metadata = Metadata {
                                    name: json["name"].as_str().unwrap_or("unknown").to_string(),
                                    description: json["description"].as_str().unwrap_or("").to_string(),
                                    version: json["version"].as_str().unwrap_or("1.0.0").to_string(),
                                };
                                
                                let mut tools = Vec::new();
                                if let Some(tools_arr) = json["tools"].as_array() {
                                    for t in tools_arr {
                                        let name = t["name"].as_str().unwrap_or("").to_string();
                                        if name.is_empty() { continue; }
                                        
                                        let tool_def = ToolDefinition {
                                            tool_type: "function".to_string(),
                                            function: crate::api::types::FunctionDefinition {
                                                name: name.clone(),
                                                description: t["description"].as_str().unwrap_or("").to_string(),
                                                parameters: t["parameters"].clone(),
                                            },
                                        };
                                        
                                        tools.push(Arc::new(ScriptTool {
                                            name,
                                            definition: tool_def,
                                            command: t["command"].as_str().unwrap_or("").to_string(),
                                            working_dir: skill_path.to_string_lossy().to_string(),
                                        }) as Arc<dyn Tool>);
                                    }
                                }
                                
                                self.register_skill(Arc::new(ScriptSkill {
                                    metadata,
                                    tools,
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn register_skill(&mut self, skill: Arc<dyn Skill>) {
        for tool in skill.tools() {
            self.tools.insert(tool.name().to_string(), tool);
        }
        self.skills.push(skill);
    }

    pub fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn all_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }
}

/// A skill loaded dynamically from a skill.json and associated scripts
pub struct ScriptSkill {
    metadata: Metadata,
    tools: Vec<Arc<dyn Tool>>,
}

impl Skill for ScriptSkill {
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }
    
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }
}

pub struct ScriptTool {
    name: String,
    definition: ToolDefinition,
    command: String,
    working_dir: String,
}

#[async_trait]
impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    
    async fn execute(
        &self, 
        args: &serde_json::Value, 
        _task_manager: &mut TaskManager
    ) -> Result<(String, String)> {
        // Replace placeholders in command with args
        let mut final_command = self.command.clone();
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                let placeholder = format!("{{{{{}}}}}", k);
                let val_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                final_command = final_command.replace(&placeholder, &val_str);
            }
        }

        // Execute the command
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&final_command)
            .current_dir(&self.working_dir)
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        
        if output.status.success() {
            Ok((stdout.clone(), format!("Executed tool {}: {}", self.name, stdout.chars().take(50).collect::<String>())))
        } else {
            Ok((format!("Error: {}\n{}", stderr, stdout), format!("Failed to execute tool {}", self.name)))
        }
    }
}

/// The bundled core skill providing basic file and system tools
struct CoreSkill;

impl Skill for CoreSkill {
    fn metadata(&self) -> Metadata {
        Metadata {
            name: "core".to_string(),
            description: "Essential file, shell, and task tools".to_string(),
            version: "1.0.0".to_string(),
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(file_edit::ReadFileTool),
            Arc::new(file_edit::WriteFileTool),
            Arc::new(file_edit::EditFileTool),
            Arc::new(file_edit::ListDirectoryTool),
            Arc::new(shell::ShellCommandTool),
            Arc::new(web::WebFetchTool),
            Arc::new(web::WebSearchTool),
            Arc::new(task::CreateTaskTool),
            Arc::new(task::UpdateTaskTool),
        ]
    }
}

/// Legacy wrapper for the agent loop to use the new registry system
pub async fn execute_tool(
    name: &str,
    arguments: &str,
    task_manager: &mut TaskManager,
    working_dir: Option<&str>,
) -> (String, ActivityEntry) {
    let registry = SkillRegistry::new(working_dir);
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::json!({}));

    if let Some(tool) = registry.get_tool(name) {
        match tool.execute(&args, task_manager).await {
            Ok((result, summary)) => (
                result,
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary,
                    status: ActivityStatus::Success,
                    timestamp: chrono::Utc::now(),
                },
            ),
            Err(e) => (
                format!("Error: {}", e),
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary: format!("{} failed", name),
                    status: ActivityStatus::Failure,
                    timestamp: chrono::Utc::now(),
                },
            ),
        }
    } else {
        (
            format!("Unknown tool: {}", name),
            ActivityEntry {
                tool_name: name.to_string(),
                summary: format!("unknown: {}", name),
                status: ActivityStatus::Failure,
                timestamp: chrono::Utc::now(),
            },
        )
    }
}

/// Build all tool definitions for the DeepSeek API request
pub fn all_tool_definitions(working_dir: Option<&str>) -> Vec<ToolDefinition> {
    SkillRegistry::new(working_dir).all_definitions()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_skill_loading() -> Result<()> {
        let dir = tempdir()?;
        let skill_path = dir.path().join("test_skill");
        fs::create_dir_all(&skill_path)?;
        
        let config = serde_json::json!({
            "name": "test",
            "description": "test skill",
            "version": "1.0.0",
            "tools": [{
                "name": "test_tool",
                "description": "test tool",
                "parameters": {"type": "object", "properties": {}},
                "command": "echo 'hello'"
            }]
        });
        
        fs::write(skill_path.join("skill.json"), serde_json::to_string(&config)?)?;
        
        let mut registry = SkillRegistry {
            skills: Vec::new(),
            tools: HashMap::new(),
        };
        
        registry.load_skills_from_dir(dir.path());
        
        assert!(registry.get_tool("test_tool").is_some());
        let tool = registry.get_tool("test_tool").unwrap();
        let (res, _) = tool.execute(&serde_json::json!({}), &mut TaskManager::new()).await?;
        assert_eq!(res.trim(), "hello");
        
        Ok(())
    }
}
