use crate::config::McpServerConfig;
use crate::mcp::client::McpClient;
use crate::mcp::types::McpToolDefinition;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct McpManager {
    clients: Mutex<HashMap<String, Arc<Mutex<McpClient>>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_client(&self, config: &McpServerConfig) -> Result<Arc<Mutex<McpClient>>> {
        let mut clients = self.clients.lock().await;

        if let Some(client) = clients.get(&config.name) {
            return Ok(client.clone());
        }

        let mut command = config.command.clone();
        let mut args = config.args.clone();

        if config.auto_install {
            // If auto_install is true, we use npx -y to run the package
            // This is a common pattern for MCP servers
            let original_command = command.clone();
            command = "npx".to_string();
            let mut npx_args = vec!["-y".to_string(), original_command];
            npx_args.extend(args);
            args = npx_args;
        }

        let client = McpClient::spawn(&command, &args).await?;
        let shared_client = Arc::new(Mutex::new(client));
        clients.insert(config.name.clone(), shared_client.clone());

        Ok(shared_client)
    }

    pub async fn list_all_tools(&self, configs: &[McpServerConfig]) -> Result<Vec<(String, McpToolDefinition)>> {
        let mut all_tools = Vec::new();
        for config in configs {
            if !config.enabled {
                continue;
            }
            
            match self.get_client(config).await {
                Ok(client_mutex) => {
                    let mut client = client_mutex.lock().await;
                    match client.list_tools().await {
                        Ok(tools) => {
                            for tool in tools {
                                all_tools.push((config.name.clone(), tool));
                            }
                        }
                        Err(e) => tracing::error!("Failed to list tools for MCP server {}: {}", config.name, e),
                    }
                }
                Err(e) => tracing::error!("Failed to connect to MCP server {}: {}", config.name, e),
            }
        }
        Ok(all_tools)
    }
}
