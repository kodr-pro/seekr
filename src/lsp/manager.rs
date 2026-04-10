use crate::lsp::client::LspClient;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct LspManager {
    clients: Mutex<HashMap<String, Arc<Mutex<LspClient>>>>,
    working_dir: PathBuf,
}

impl LspManager {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            working_dir,
        }
    }

    pub async fn get_client(&self, language: &str, command: &str, args: &[&str]) -> Result<Arc<Mutex<LspClient>>> {
        let mut clients = self.clients.lock().await;
        
        if let Some(client) = clients.get(language) {
            return Ok(client.clone());
        }

        let client = LspClient::spawn(command, args, &self.working_dir).await?;
        let shared_client = Arc::new(Mutex::new(client));
        clients.insert(language.to_string(), shared_client.clone());
        
        Ok(shared_client)
    }
}
