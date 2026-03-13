// manager.rs - System-wide manager for sessions and resources
//
// Centralizes logic for session management, configuration, and shared tools.

use anyhow::{Result, Context};
use crate::config::AppConfig;
use crate::session::{Session, SessionMetadata};
use tokio::sync::RwLock;

/// Managed application state
pub struct SeekrManager {
    pub config: AppConfig,
    active_sessions: RwLock<Vec<SessionMetadata>>,
}

impl SeekrManager {
    /// Create a new manager with the given config
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            active_sessions: RwLock::new(Vec::new()),
        }
    }

    /// Load all sessions from disk
    pub async fn load_sessions(&self) -> Result<()> {
        let sessions = Session::list_all()?;
        let mut active = self.active_sessions.write().await;
        *active = sessions;
        Ok(())
    }

    /// List all available sessions
    pub async fn list_sessions(&self) -> Vec<SessionMetadata> {
        self.active_sessions.read().await.clone()
    }

    /// Resume a session by ID
    pub fn resume_session(&self, id: &str) -> Result<Session> {
        Session::load(id)
            .with_context(|| format!("Failed to resume session {}", id))
    }

    /// Create a new session
    pub fn create_session(&self, title: String) -> Session {
        let id = uuid::Uuid::new_v4().to_string();
        Session::new(id, title)
    }

    /// Delete a session
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        let dir = Session::sessions_dir()?;
        let path = dir.join(format!("{}.json", id));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        
        // Update cached list
        let mut active = self.active_sessions.write().await;
        active.retain(|s| s.id != id);
        
        Ok(())
    }
    
    /// Get the system-wide tool registry
    pub fn tool_registry(&self) -> crate::tools::SkillRegistry {
        crate::tools::SkillRegistry::new(Some(&self.config.agent.working_directory))
    }
}
