use crate::api::types::ToolDefinition;
use crate::mcp::types::ResourceData;
use crate::tools::{ExecutionContext, Tool};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;

pub struct McpReadResourceTool;

#[async_trait]
impl Tool for McpReadResourceTool {
    fn name(&self) -> &str {
        "mcp_read_resource"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::api::types::FunctionDefinition {
                name: self.name().to_string(),
                description: "Read the content of a resource from an MCP server.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "The name of the MCP server providing the resource."
                        },
                        "uri": {
                            "type": "string",
                            "description": "The URI of the resource to read."
                        }
                    },
                    "required": ["server_name", "uri"]
                }),
            },
        }
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let server_name = args["server_name"].as_str().ok_or_else(|| anyhow!("Missing server_name"))?;
        let uri = args["uri"].as_str().ok_or_else(|| anyhow!("Missing uri"))?;

        let summary = format!("Reading MCP resource {} from {}", uri, server_name);
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let config = context.config.mcp_servers.iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| anyhow!("MCP server {} not found", server_name))?;

        let client_mutex = context.mcp_manager.get_client(config, Some(context.task_manager.clone())).await?;
        let mut client = client_mutex.lock().await;
        let result = client.read_resource(uri).await?;

        let mut output = String::new();
        for content in result.contents {
            match content.content {
                ResourceData::Text { text } => {
                    output.push_str(&text);
                    output.push('\n');
                }
                ResourceData::Blob { .. } => {
                    output.push_str("[Binary Data Block]\n");
                }
            }
        }

        Ok((output, summary))
    }
}

pub struct McpListResourcesTool;

#[async_trait]
impl Tool for McpListResourcesTool {
    fn name(&self) -> &str {
        "mcp_list_resources"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::api::types::FunctionDefinition {
                name: self.name().to_string(),
                description: "List available resources from an MCP server.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "The name of the MCP server to list resources from."
                        }
                    },
                    "required": ["server_name"]
                }),
            },
        }
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let server_name = args["server_name"].as_str().ok_or_else(|| anyhow!("Missing server_name"))?;
        
        let summary = format!("Listing MCP resources from {}", server_name);
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let config = context.config.mcp_servers.iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| anyhow!("MCP server {} not found", server_name))?;

        let client_mutex = context.mcp_manager.get_client(config, Some(context.task_manager.clone())).await?;
        let mut client = client_mutex.lock().await;
        let resources = client.list_resources().await?;

        let mut output = format!("Resources from {}:\n", server_name);
        for res in resources {
            output.push_str(&format!("- {}: {} ({})\n", res.name, res.uri, res.description.unwrap_or_default()));
        }

        Ok((output, summary))
    }
}
