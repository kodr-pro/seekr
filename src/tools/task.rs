// tools/task.rs - Task management tools
//
// Provides create_task and update_task for the agent to track progress
// on multi-step operations. Tasks are displayed in the TUI task panel.

use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::Tool;
use anyhow::{Result, anyhow};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Possible task statuses
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

impl TaskStatus {
    /// Parse status from a string
    pub fn from_str_loose(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pending" => TaskStatus::Pending,
            "in_progress" | "in-progress" | "inprogress" => TaskStatus::InProgress,
            "completed" | "complete" | "done" => TaskStatus::Completed,
            "failed" | "error" => TaskStatus::Failed,
            _ => TaskStatus::Pending,
        }
    }

    /// Icon for display in the TUI
    #[allow(dead_code)]
    pub fn icon(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "○",
            TaskStatus::InProgress => "●",
            TaskStatus::Completed => "✓",
            TaskStatus::Failed => "✗",
        }
    }
}

/// A tracked task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub tool_name: String,
    pub summary: String,
}

pub type InputSender = tokio::sync::mpsc::UnboundedSender<String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskManager {
    pub tasks: Vec<Task>,
    pub activities: Vec<ActivityEntry>,
    next_id: usize,
    #[serde(skip)]
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::agent::AgentEvent>>,
    #[serde(skip)]
    pub input_tx: Arc<Mutex<Option<InputSender>>>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            activities: Vec::new(),
            next_id: 1,
            event_tx: None,
            input_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn log_activity(&mut self, tool_name: &str, summary: &str) {
        let activity = ActivityEntry {
            tool_name: tool_name.to_string(),
            summary: summary.to_string(),
        };
        self.activities.push(activity.clone());
        if let Some(ref tx) = self.event_tx {
            tx.send(crate::agent::AgentEvent::Activity(activity)).ok();
        }
    }

    pub fn set_input_tx(&self, tx: InputSender) {
        if let Ok(mut lock) = self.input_tx.try_lock() {
            *lock = Some(tx);
        }
    }

    pub fn with_sender(mut self, tx: tokio::sync::mpsc::UnboundedSender<crate::agent::AgentEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Create a new task and return its ID
    pub fn create_task(&mut self, title: &str, status: Option<&str>) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let task = Task {
            id,
            title: title.to_string(),
            status: status
                .map(|s| TaskStatus::from_str_loose(s))
                .unwrap_or(TaskStatus::Pending),
        };
        self.tasks.push(task);
        id
    }

    /// Update an existing task's status. Returns Ok with a message, or Err if not found.
    pub fn update_task(&mut self, task_id: usize, status: &str) -> Result<String, String> {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::from_str_loose(status);
            Ok(format!("Task {} updated to {}", task_id, task.status))
        } else {
            Err(format!("Task {} not found", task_id))
        }
    }
}

// --- Tools ---

pub struct CreateTaskTool;

#[async_trait]
impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Create a task to track progress on a multi-step operation. Tasks appear in the task panel.".to_string(),
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
        }
    }

    async fn execute(&self, args: &serde_json::Value, task_manager: &mut TaskManager) -> Result<(String, String)> {
        let title = args["title"].as_str().ok_or_else(|| anyhow!("Missing title"))?;
        let status = args["status"].as_str();
        let id = task_manager.create_task(title, status);
        let summary = format!("create_task #{}", id);
        Ok((format!("Created task #{}: {}", id, title), summary))
    }
}

pub struct UpdateTaskTool;

#[async_trait]
impl Tool for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
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
        }
    }

    async fn execute(&self, args: &serde_json::Value, task_manager: &mut TaskManager) -> Result<(String, String)> {
        let task_id = args["task_id"].as_u64().ok_or_else(|| anyhow!("Missing task_id"))? as usize;
        let status = args["status"].as_str().ok_or_else(|| anyhow!("Missing status"))?;
        let summary = format!("update_task #{}", task_id);
        match task_manager.update_task(task_id, status) {
            Ok(msg) => Ok((msg, summary)),
            Err(e) => Err(anyhow!(e)),
        }
    }
}
