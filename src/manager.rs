use crate::config::AppConfig;
use crate::session::{Session, SessionMetadata};
use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::tools::SkillRegistry;
use std::sync::Arc;

pub struct SeekrManager {
    pub config: AppConfig,
    active_sessions: RwLock<Vec<SessionMetadata>>,
    tool_registry: Arc<SkillRegistry>,
}

impl SeekrManager {
    pub fn new(config: AppConfig) -> Self {
        let tool_registry = Arc::new(SkillRegistry::new(Some(&config.agent.working_directory)));
        Self {
            config,
            active_sessions: RwLock::new(Vec::new()),
            tool_registry,
        }
    } // new

    pub async fn load_sessions(&self) -> Result<()> {
        let sessions = Session::list_all()?;
        let mut active = self.active_sessions.write().await;
        *active = sessions;
        Ok(())
    } // load_sessions

    pub async fn list_sessions(&self) -> Vec<SessionMetadata> {
        self.active_sessions.read().await.clone()
    } // list_sessions

    pub fn resume_session(&self, id: &str) -> Result<Session> {
        let mut session =
            Session::load(id).with_context(|| format!("Failed to resume session {}", id))?;
        session.tool_registry = Some(self.tool_registry.clone());
        Ok(session)
    } // resume_session

    pub fn create_session(&self, title: String) -> Session {
        let id = uuid::Uuid::new_v4().to_string();
        let mut session = Session::new(id, title);
        session.tool_registry = Some(self.tool_registry.clone());
        session
    } // create_session

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        let dir = Session::sessions_dir()?;
        let path = dir.join(format!("{}.json", id));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }

        let mut active = self.active_sessions.write().await;
        active.retain(|s| s.id != id);

        Ok(())
    } // delete_session

    pub fn tool_registry(&self) -> Arc<SkillRegistry> {
        self.tool_registry.clone()
    } // tool_registry
} // impl SeekrManager
