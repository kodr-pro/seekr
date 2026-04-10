use crate::config::McpServerConfig;
use crate::mcp::client::McpClient;
use crate::mcp::types::{McpToolDefinition, Prompt, Resource};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct McpServerMetadata {
    pub tools: Vec<McpToolDefinition>,
    pub resources: Vec<Resource>,
    pub prompts: Vec<Prompt>,
}

pub struct McpManager {
    clients: Mutex<HashMap<String, Arc<Mutex<McpClient>>>>,
    pub metadata: Mutex<HashMap<String, McpServerMetadata>>,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            metadata: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_client(
        &self,
        config: &McpServerConfig,
        task_manager: Option<crate::tools::TaskManager>,
    ) -> Result<Arc<Mutex<McpClient>>> {
        let mut clients = self.clients.lock().await;

        if let Some(client) = clients.get(&config.name) {
            return Ok(client.clone());
        }

        let mut command = config.command.clone();
        let mut args = config.args.clone();

        if config.auto_install {
            let original_command = command.clone();
            command = "npx".to_string();
            let mut npx_args = vec!["-y".to_string(), original_command];
            npx_args.extend(args);
            args = npx_args;
        }

        let client = McpClient::spawn(&command, &args).await?;
        let shared_client = Arc::new(Mutex::new(client));

        // Spawn log forwarder
        if let Some(tm) = task_manager {
            let client_clone = shared_client.clone();
            let server_name = config.name.clone();
            tokio::spawn(async move {
                let rx_mutex = {
                    let client = client_clone.lock().await;
                    client.notification_rx.clone()
                };
                let mut rx = rx_mutex.lock().await;
                while let Some(notif) = rx.recv().await {
                    if notif.method == "notifications/message"
                        && let Ok(msg) = serde_json::from_value::<
                            crate::mcp::types::LoggingMessageNotification,
                        >(notif.params)
                    {
                        tm.log_activity(
                            &server_name,
                            &msg.data.to_string(),
                            crate::tools::task::ActivityStatus::Starting, // Use Starting as a generic "In Progress/Log" status
                            None,
                            None,
                        );
                    }
                }
            });
        }

        clients.insert(config.name.clone(), shared_client.clone());

        Ok(shared_client)
    }

    pub async fn list_all_tools(
        &self,
        configs: &[McpServerConfig],
        task_manager: Option<crate::tools::TaskManager>,
    ) -> Result<Vec<(String, McpToolDefinition)>> {
        let mut all_tools = Vec::new();
        for config in configs {
            if !config.enabled {
                continue;
            }

            match self.get_client(config, task_manager.clone()).await {
                Ok(client_mutex) => {
                    let mut client = client_mutex.lock().await;
                    match client.list_tools().await {
                        Ok(tools) => {
                            let mut meta = self.metadata.lock().await;
                            let entry =
                                meta.entry(config.name.clone())
                                    .or_insert(McpServerMetadata {
                                        tools: Vec::new(),
                                        resources: Vec::new(),
                                        prompts: Vec::new(),
                                    });
                            entry.tools = tools.clone();
                            for tool in tools {
                                all_tools.push((config.name.clone(), tool));
                            }
                        }
                        Err(e) => tracing::error!(
                            "Failed to list tools for MCP server {}: {}",
                            config.name,
                            e
                        ),
                    }
                }
                Err(e) => tracing::error!("Failed to connect to MCP server {}: {}", config.name, e),
            }
        }
        Ok(all_tools)
    }

    pub async fn list_all_resources(
        &self,
        configs: &[McpServerConfig],
        task_manager: Option<crate::tools::TaskManager>,
    ) -> Result<Vec<(String, Resource)>> {
        let mut all_resources = Vec::new();
        for config in configs {
            if !config.enabled {
                continue;
            }
            match self.get_client(config, task_manager.clone()).await {
                Ok(client_mutex) => {
                    let mut client = client_mutex.lock().await;
                    match client.list_resources().await {
                        Ok(resources) => {
                            let mut meta = self.metadata.lock().await;
                            let entry =
                                meta.entry(config.name.clone())
                                    .or_insert(McpServerMetadata {
                                        tools: Vec::new(),
                                        resources: Vec::new(),
                                        prompts: Vec::new(),
                                    });
                            entry.resources = resources.clone();
                            for res in resources {
                                all_resources.push((config.name.clone(), res));
                            }
                        }
                        Err(e) => tracing::error!(
                            "Failed to list resources for MCP server {}: {}",
                            config.name,
                            e
                        ),
                    }
                }
                Err(e) => tracing::error!("Failed to connect to MCP server {}: {}", config.name, e),
            }
        }
        Ok(all_resources)
    }

    pub async fn list_all_prompts(
        &self,
        configs: &[McpServerConfig],
        task_manager: Option<crate::tools::TaskManager>,
    ) -> Result<Vec<(String, Prompt)>> {
        let mut all_prompts = Vec::new();
        for config in configs {
            if !config.enabled {
                continue;
            }
            match self.get_client(config, task_manager.clone()).await {
                Ok(client_mutex) => {
                    let mut client = client_mutex.lock().await;
                    match client.list_prompts().await {
                        Ok(prompts) => {
                            let mut meta = self.metadata.lock().await;
                            let entry =
                                meta.entry(config.name.clone())
                                    .or_insert(McpServerMetadata {
                                        tools: Vec::new(),
                                        resources: Vec::new(),
                                        prompts: Vec::new(),
                                    });
                            entry.prompts = prompts.clone();
                            for p in prompts {
                                all_prompts.push((config.name.clone(), p));
                            }
                        }
                        Err(e) => tracing::error!(
                            "Failed to list prompts for MCP server {}: {}",
                            config.name,
                            e
                        ),
                    }
                }
                Err(e) => tracing::error!("Failed to connect to MCP server {}: {}", config.name, e),
            }
        }
        Ok(all_prompts)
    }
}
