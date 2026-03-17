use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::Tool;
use anyhow::{Result, anyhow};
use serde_json::json;
use std::sync::{Arc, Mutex};

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
} // fmt

impl TaskStatus {
    pub fn from_str_loose(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "pending" => TaskStatus::Pending,
            "in_progress" | "in-progress" | "inprogress" => TaskStatus::InProgress,
            "completed" | "complete" | "done" => TaskStatus::Completed,
            "failed" | "error" => TaskStatus::Failed,
            _ => TaskStatus::Pending,
        }
    } // from_str_loose

    pub fn icon(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "○",
            TaskStatus::InProgress => "●",
            TaskStatus::Completed => "✓",
            TaskStatus::Failed => "✗",
        }
    } // icon
} // impl TaskStatus

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActivityStatus {
    Starting,
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub tool_name: String,
    pub summary: String,
    pub status: ActivityStatus,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub thread_id: Option<usize>,
    pub total_threads: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskManagerState {
    pub tasks: Vec<Task>,
    pub activities: Vec<ActivityEntry>,
    pub live_activities: std::collections::HashMap<usize, ActivityEntry>,
    pub next_id: usize,
}

#[derive(Debug, Clone)]
pub struct TaskManager {
    state: Arc<Mutex<TaskManagerState>>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::agent::AgentEvent>>,
}

impl Serialize for TaskManager {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let state = self.state.lock().unwrap();
        state.serialize(serializer)
    }
} // serialize

impl<'de> Deserialize<'de> for TaskManager {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let state = TaskManagerState::deserialize(deserializer)?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            event_tx: None,
        })
    }
} // deserialize

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
} // default

impl TaskManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TaskManagerState {
                tasks: Vec::new(),
                activities: Vec::new(),
                live_activities: std::collections::HashMap::new(),
                next_id: 1,
            })),
            event_tx: None,
        }
    } // new

    pub fn live_activities(&self) -> Vec<ActivityEntry> {
        let state = self.state.lock().unwrap();
        let mut live: Vec<_> = state.live_activities.values().cloned().collect();
        live.sort_by_key(|a| a.thread_id.unwrap_or(0));
        live
    } // live_activities

    pub fn activities(&self) -> Vec<ActivityEntry> {
        self.state.lock().unwrap().activities.clone()
    } // activities

    pub fn tasks(&self) -> Vec<Task> {
        self.state.lock().unwrap().tasks.clone()
    } // tasks

    pub fn log_activity(
        &self, 
        tool_name: &str, 
        summary: &str, 
        status: ActivityStatus,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) {
        let activity = ActivityEntry {
            tool_name: tool_name.to_string(),
            summary: summary.to_string(),
            status: status.clone(),
            timestamp: chrono::Utc::now(),
            thread_id,
            total_threads,
        };
        if let Ok(mut state) = self.state.lock() {
            state.activities.push(activity.clone());
            if let Some(tid) = thread_id {
                if status == ActivityStatus::Starting {
                    state.live_activities.insert(tid, activity.clone());
                } else {
                    state.live_activities.remove(&tid);
                }
            }
        }
        if let Some(ref tx) = self.event_tx {
            tx.send(crate::agent::AgentEvent::Activity(activity)).ok();
        }
    } // log_activity

    pub fn with_sender(mut self, tx: tokio::sync::mpsc::UnboundedSender<crate::agent::AgentEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    } // with_sender

    pub fn create_task(&self, title: &str, status: Option<&str>) -> usize {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        
        let task = Task {
            id,
            title: title.to_string(),
            status: status
                .map(TaskStatus::from_str_loose)
                .unwrap_or(TaskStatus::Pending),
        };
        
        state.tasks.push(task.clone());
        
        if let Some(ref tx) = self.event_tx {
            tx.send(crate::agent::AgentEvent::TaskCreated(task)).ok();
        }
        
        id
    } // create_task

    pub fn update_task(&self, task_id: usize, status: &str) -> Result<String, String> {
        let mut state = self.state.lock().map_err(|_| "Lock poisoned".to_string())?;
        if let Some(task) = state.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::from_str_loose(status);
            let updated_task = task.clone();
            
            if let Some(ref tx) = self.event_tx {
                tx.send(crate::agent::AgentEvent::TaskUpdated(updated_task)).ok();
            }
            
            Ok(format!("Task {} updated to {}", task_id, task.status))
        } else {
            Err(format!("Task {} not found", task_id))
        }
    } // update_task
} // impl TaskManager

pub struct CreateTaskTool;

#[async_trait]
impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    } // name

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Create a task to track progress on a multi-step operation.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Short task title" },
                        "status": { "type": "string", "description": "Task status: pending, in_progress, completed, failed" }
                    },
                    "required": ["title"]
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
        let title = args["title"].as_str().ok_or_else(|| anyhow!("Missing title"))?;
        let status = args["status"].as_str();
        let task_id = task_manager.create_task(title, status);
        let summary = format!("Created task {}: {}", task_id, title);
        task_manager.log_activity(self.name(), &summary, ActivityStatus::Success, thread_id, total_threads);
        Ok((format!("Created task ID: {}", task_id), summary))
    } // execute
} // impl CreateTaskTool

pub struct UpdateTaskTool;

#[async_trait]
impl Tool for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    } // name

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Update the status of an existing task.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer", "description": "Task ID" },
                        "status": { "type": "string", "description": "New status: pending, in_progress, completed, failed" }
                    },
                    "required": ["id", "status"]
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
        let id_raw = args["id"].as_u64().ok_or_else(|| anyhow!("Missing id"))? as usize;
        let status = args["status"].as_str().ok_or_else(|| anyhow!("Missing status"))?;
        
        task_manager.update_task(id_raw, status)
            .map_err(|e| anyhow!(e))?;
        let summary = format!("Updated task {} to {}", id_raw, status);
        task_manager.log_activity(self.name(), &summary, ActivityStatus::Success, thread_id, total_threads);
        Ok((format!("Successfully updated task to {}", status), summary))
    } // execute
} // impl UpdateTaskTool

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use crate::agent::AgentEvent;

    #[tokio::test]
    async fn test_task_sync() {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let task_manager = TaskManager::new().with_sender(event_tx);
        
        let task_id = task_manager.create_task("Test Task", Some("in_progress"));
        assert_eq!(task_manager.tasks().len(), 1);
        assert_eq!(task_manager.tasks()[0].status, TaskStatus::InProgress);
        
        if let Some(AgentEvent::TaskCreated(task)) = event_rx.recv().await {
            assert_eq!(task.id, task_id);
        } else {
            panic!("Expected TaskCreated event");
        }
        
        task_manager.update_task(task_id, "completed").unwrap();
        assert_eq!(task_manager.tasks()[0].status, TaskStatus::Completed);
        
        if let Some(AgentEvent::TaskUpdated(task)) = event_rx.recv().await {
            assert_eq!(task.id, task_id);
            assert_eq!(task.status, TaskStatus::Completed);
        } else {
            panic!("Expected TaskUpdated event");
        }
    } // test_task_sync
} // tests
