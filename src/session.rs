use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::api::types::ChatMessage;
use crate::tools::task::TaskManager;
use crate::tools::SkillRegistry;
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<ChatMessage>,
    pub task_manager: TaskManager,

    #[serde(skip)]
    pub tool_registry: Option<Arc<SkillRegistry>>,
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
            tool_registry: None,
        }
    } // new

    pub fn sessions_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not determine config directory")?;
        Ok(config_dir.join("seekr").join("sessions"))
    } // sessions_dir

    pub fn file_path(&self) -> Result<PathBuf> {
        Ok(Self::sessions_dir()?.join(format!("{}.json", self.id)))
    } // file_path

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = self.file_path()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create sessions directory: {}", parent.display())
            })?;
        }

        let contents = serde_json::to_string_pretty(self).context("Failed to serialize session")?;
        fs::write(&path, contents)
            .with_context(|| format!("Failed to write session to {}", path.display()))?;

        Ok(())
    } // save

    pub fn load(id: &str) -> Result<Self> {
        let dir = Self::sessions_dir()?;
        let path = dir.join(format!("{}.json", id));
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;
        let mut session: Session =
            serde_json::from_str(&contents).with_context(|| "Failed to parse session JSON")?;
        session.tool_registry = None;
        Ok(session)
    } // load

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
    } // list_all
} // impl Session

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
    } // test_session_new
} // tests
