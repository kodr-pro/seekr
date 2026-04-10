pub mod file_edit;
pub mod shell;
pub mod task;
pub mod web;
pub mod agent;
pub mod lsp;
pub mod mcp;
pub mod mcp_resource;
pub mod mcp_prompt;
pub mod parser;

use crate::api::types::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use crate::tools::task::{ActivityEntry, ActivityStatus, TaskManager};
use crate::config::{AppConfig, McpServerConfig};
use crate::lsp::LspManager;
use crate::mcp::McpManager;
use crate::tools::mcp::McpTool;
use crate::tools::mcp_resource::{McpReadResourceTool, McpListResourcesTool};
use crate::tools::mcp_prompt::{McpGetPromptTool, McpListPromptsTool};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    pub version: String,
}

pub struct ExecutionContext {
    pub task_manager: TaskManager,
    pub registry: Arc<SkillRegistry>,
    pub config: AppConfig,
    pub lsp_manager: Arc<LspManager>,
    pub mcp_manager: Arc<McpManager>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)>;
} // Tool

pub trait Skill: Send + Sync {
    fn metadata(&self) -> Metadata;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
} // Skill

#[derive(Clone)]
pub struct SkillRegistry {
    pub skills: Arc<Mutex<Vec<Arc<dyn Skill>>>>,
    pub tools: Arc<Mutex<HashMap<String, Arc<dyn Tool>>>>,
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let skills = self.skills.lock().unwrap();
        let tools = self.tools.lock().unwrap();
        f.debug_struct("SkillRegistry")
            .field("skills_count", &skills.len())
            .field("tools_count", &tools.len())
            .finish()
    }
} // fmt

impl SkillRegistry {
    pub fn new(working_dir: Option<&str>) -> Self {
        let registry = Self {
            skills: Arc::new(Mutex::new(Vec::new())),
            tools: Arc::new(Mutex::new(HashMap::new())),
        };

        registry.register_skill(Arc::new(CoreSkill));

        if let Some(config_dir) = dirs::config_dir() {
            let global_skills_path = config_dir.join("seekr").join("skills");
            if global_skills_path.exists() {
                registry.load_skills_from_dir(&global_skills_path);
            }
        }

        if let Some(wd) = working_dir {
            let expanded_wd = shellexpand::tilde(wd);
            let local_skills_path = std::path::Path::new(expanded_wd.as_ref())
                .join(".seekr")
                .join("skills");
            if local_skills_path.exists() {
                registry.load_skills_from_dir(&local_skills_path);
            }
        }

        registry
    } // new

    fn load_skills_from_dir(&self, path: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let skill_path = entry.path();
                if skill_path.is_dir() {
                    // Try skill.json (Legacy)
                    let config_path = skill_path.join("skill.json");
                    if config_path.exists()
                        && let Ok(content) = std::fs::read_to_string(&config_path)
                        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                    {
                        self.load_json_skill(json, &skill_path);
                        continue;
                    }

                    // Try skill.md (New)
                    let md_path = skill_path.join("skill.md");
                    if md_path.exists()
                        && let Ok(content) = std::fs::read_to_string(&md_path)
                        && let Ok((metadata, tools)) = parser::parse_markdown_skill(&content, &skill_path.to_string_lossy())
                    {
                        self.register_skill(Arc::new(ScriptSkill { metadata, tools }));
                    }
                } else if skill_path.extension().and_then(|e| e.to_str()) == Some("md") {
                    // Direct .md file as a skill
                    let parent = skill_path.parent().unwrap_or(std::path::Path::new("."));
                    if let Ok(content) = std::fs::read_to_string(&skill_path)
                        && let Ok((metadata, tools)) = parser::parse_markdown_skill(&content, &parent.to_string_lossy())
                    {
                        self.register_skill(Arc::new(ScriptSkill { metadata, tools }));
                    }
                }
            }
        }
    } // load_skills_from_dir

    fn load_json_skill(&self, json: serde_json::Value, skill_path: &std::path::Path) {
        let metadata = Metadata {
            name: json["name"].as_str().unwrap_or("unknown").to_string(),
            description: json["description"].as_str().unwrap_or("").to_string(),
            version: json["version"].as_str().unwrap_or("1.0.0").to_string(),
        };

        let mut tools = Vec::new();
        if let Some(tools_arr) = json["tools"].as_array() {
            for t in tools_arr {
                let name = t["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }

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

        self.register_skill(Arc::new(ScriptSkill { metadata, tools }));
    }

    pub fn register_skill(&self, skill: Arc<dyn Skill>) {
        let mut tools_guard = self.tools.lock().unwrap();
        for tool in skill.tools() {
            tools_guard.insert(tool.name().to_string(), tool);
        }
        self.skills.lock().unwrap().push(skill);
    } // register_skill

    pub async fn load_mcp_tools(&self, mcp_manager: &McpManager, configs: &[McpServerConfig], task_manager: Option<TaskManager>) -> Result<()> {
        let mcp_tools = mcp_manager.list_all_tools(configs, task_manager).await?;
        let mut tools = self.tools.lock().unwrap();
        for (server_name, tool_def) in mcp_tools {
            let tool = Arc::new(McpTool::new(server_name, tool_def));
            tools.insert(tool.name().to_string(), tool);
        }
        Ok(())
    }

    pub fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.lock().unwrap().get(name).cloned()
    } // get_tool

    pub fn all_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.lock().unwrap().values().map(|t| t.definition()).collect()
    } // all_definitions
} // impl SkillRegistry

pub struct ScriptSkill {
    metadata: Metadata,
    tools: Vec<Arc<dyn Tool>>,
}

impl Skill for ScriptSkill {
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    } // metadata

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    } // tools
} // impl Skill for ScriptSkill

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
    } // name

    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    } // definition

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let summary = format!("script_tool {}", self.name);
        context.task_manager.log_activity(
            &self.name,
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let mut final_command = self.command.clone();
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                let placeholder = format!("{{{{{}}}}}", k);
                let val_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                let escaped_val = shell_escape(&val_str);
                final_command = final_command.replace(&placeholder, &escaped_val);
            }
        }

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&final_command)
            .current_dir(&self.working_dir)
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok((
                stdout.clone(),
                format!(
                    "Executed tool {}: {}",
                    self.name,
                    truncate(stdout.trim(), 50)
                ),
            ))
        } else {
            Ok((
                format!("Error: {}\n{}", stderr, stdout),
                format!("Failed to execute tool {}", self.name),
            ))
        }
    } // execute
} // impl ScriptTool

struct CoreSkill;

impl Skill for CoreSkill {
    fn metadata(&self) -> Metadata {
        Metadata {
            name: "core".to_string(),
            description: "Essential file, shell, and task tools".to_string(),
            version: "1.0.0".to_string(),
        }
    } // metadata

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
            Arc::new(agent::SubAgentTool::new()),
            Arc::new(lsp::LspDefinitionTool),
            Arc::new(lsp::LspReferencesTool),
            Arc::new(lsp::LspHoverTool),
            Arc::new(McpReadResourceTool),
            Arc::new(McpListResourcesTool),
            Arc::new(McpGetPromptTool),
            Arc::new(McpListPromptsTool),
        ]
    } // tools
} // impl Skill for CoreSkill

pub async fn execute_tool(
    name: &str,
    args_json: &str,
    context: &ExecutionContext,
    thread_id: Option<usize>,
    total_threads: Option<usize>,
) -> (String, ActivityEntry) {
    let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::json!({}));

    let tool = match context.registry.get_tool(name) {
        Some(t) => t,
        _ => {
            context.task_manager.log_activity(
                name,
                &format!("Error: Unknown tool {}", name),
                ActivityStatus::Failure,
                thread_id,
                total_threads,
            );
            return (
                format!("Error: Unknown tool '{}'", name),
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary: format!("Unknown tool: {}", name),
                    status: ActivityStatus::Failure,
                    timestamp: chrono::Utc::now(),
                    thread_id,
                    total_threads,
                },
            );
        }
    };

    match tool
        .execute(&args, context, thread_id, total_threads)
        .await
    {
        Ok((result, summary)) => {
            context.task_manager.log_activity(
                name,
                &summary,
                ActivityStatus::Success,
                thread_id,
                total_threads,
            );
            (
                result,
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary,
                    status: ActivityStatus::Success,
                    timestamp: chrono::Utc::now(),
                    thread_id,
                    total_threads,
                },
            )
        }
        Err(e) => {
            let error_msg = format!("Error: {}", e);
            context.task_manager.log_activity(
                name,
                &error_msg,
                ActivityStatus::Failure,
                thread_id,
                total_threads,
            );
            (
                error_msg,
                ActivityEntry {
                    tool_name: name.to_string(),
                    summary: format!("Failed: {}", name),
                    status: ActivityStatus::Failure,
                    timestamp: chrono::Utc::now(),
                    thread_id,
                    total_threads,
                },
            )
        }
    }
} // execute_tool

pub fn all_tool_definitions(registry: &SkillRegistry) -> Vec<ToolDefinition> {
    registry.all_definitions()
} // all_tool_definitions

pub fn short_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
} // short_path

pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    }
} // truncate

pub fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // For POSIX shell: wrap in '', escape ' with '\''
    format!("'{}'", s.replace("'", "'\\''"))
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

        fs::write(
            skill_path.join("skill.json"),
            serde_json::to_string(&config)?,
        )?;

        let registry = SkillRegistry {
            skills: Arc::new(Mutex::new(Vec::new())),
            tools: Arc::new(Mutex::new(HashMap::new())),
        };

        registry.load_skills_from_dir(dir.path());

        assert!(registry.get_tool("test_tool").is_some());
        let tool = registry
            .get_tool("test_tool")
            .expect("test_tool should exist");
        let (res, _) = tool
            .execute(&serde_json::json!({}), &crate::tools::ExecutionContext { task_manager: TaskManager::new(), registry: std::sync::Arc::new(SkillRegistry::new(None)), config: crate::config::AppConfig::default(), lsp_manager: std::sync::Arc::new(crate::lsp::LspManager::new(std::path::PathBuf::from("/tmp"))), mcp_manager: std::sync::Arc::new(crate::mcp::McpManager::new()) }, None, None)
            .await?;
        assert_eq!(res.trim(), "hello");

        Ok(())
    } // test_skill_loading
} // tests
