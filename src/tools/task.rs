// tools/task.rs - Task management tools
//
// Provides create_task and update_task for the agent to track progress
// on multi-step operations. Tasks are displayed in the TUI task panel.

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub status: TaskStatus,
}

/// Task manager that holds the list of active tasks
#[derive(Debug, Default)]
pub struct TaskManager {
    pub tasks: Vec<Task>,
    next_id: usize,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
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
