// session.rs - Session persistence (saving/loading chat history)
//
// Manages the serialization of agent state to ~/.config/seekr/sessions/.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use chrono::{DateTime, Utc};

use crate::api::types::ChatMessage;
use crate::tools::task::TaskManager;

/// A point-in-time snapshot of the agent's state
#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<ChatMessage>,
    pub task_manager: TaskManager,
}

impl Session {
    pub fn new(id: String, title: String) -> Self {
        Self {
            id,
            title,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            messages: Vec::new(),
            task_manager: TaskManager::new(),
        }
    }

    /// Path to the sessions directory: ~/.config/seekr/sessions/
    pub fn sessions_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?;
        Ok(config_dir.join("seekr").join("sessions"))
    }

    /// Path to this specific session file
    pub fn file_path(&self) -> Result<PathBuf> {
        Ok(Self::sessions_dir()?.join(format!("{}.json", self.id)))
    }

    /// Save the session to disk
    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = self.file_path()?;
        
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create sessions directory: {}", parent.display())
            })?;
        }

        let contents = serde_json::to_string_pretty(self)
            .context("Failed to serialize session")?;
        fs::write(&path, contents).with_context(|| {
            format!("Failed to write session to {}", path.display())
        })?;
        
        Ok(())
    }

    /// Load a session from disk
    pub fn load(id: &str) -> Result<Self> {
        let dir = Self::sessions_dir()?;
        let path = dir.join(format!("{}.json", id));
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;
        let session: Session = serde_json::from_str(&contents)
            .with_context(|| "Failed to parse session JSON")?;
        Ok(session)
    }

    /// List all available sessions, sorted by update time (newest first)
    #[allow(dead_code)]
    pub fn list_all() -> Result<Vec<SessionMetadata>> {
        let dir = Self::sessions_dir()?;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let contents = fs::read_to_string(&path)?;
                if let Ok(session) = serde_json::from_str::<Session>(&contents) {
                    sessions.push(SessionMetadata {
                        id: session.id,
                        title: session.title,
                        updated_at: session.updated_at,
                    });
                }
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_new() {
        let session = Session::new("test-id".to_string(), "Test Session".to_string());
        assert_eq!(session.id, "test-id");
        assert_eq!(session.title, "Test Session");
        assert_eq!(session.messages.len(), 0);
    }
}
