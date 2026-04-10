use crate::api::types::ToolDefinition;
use crate::mcp::types::PromptContent;
use crate::tools::{ExecutionContext, Tool};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::json;

pub struct McpGetPromptTool;

#[async_trait]
impl Tool for McpGetPromptTool {
    fn name(&self) -> &str {
        "mcp_get_prompt"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::api::types::FunctionDefinition {
                name: self.name().to_string(),
                description: "Retrieve a conversation template (prompt) from an MCP server."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "The name of the MCP server providing the prompt."
                        },
                        "prompt_name": {
                            "type": "string",
                            "description": "The name of the prompt template to retrieve."
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Arguments to populate the template with."
                        }
                    },
                    "required": ["server_name", "prompt_name"]
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
        let server_name = args["server_name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing server_name"))?;
        let prompt_name = args["prompt_name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing prompt_name"))?;
        let arguments = args["arguments"].clone();

        let summary = format!("Getting MCP prompt {} from {}", prompt_name, server_name);
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let config = context
            .config
            .mcp_servers
            .iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| anyhow!("MCP server {} not found", server_name))?;

        let client_mutex = context
            .mcp_manager
            .get_client(config, Some(context.task_manager.clone()))
            .await?;
        let mut client = client_mutex.lock().await;
        let result = client.get_prompt(prompt_name, arguments).await?;

        let mut output = format!("Prompt: {}\n", prompt_name);
        if let Some(desc) = result.description {
            output.push_str(&format!("Description: {}\n", desc));
        }
        output.push_str("---\n");

        for msg in result.messages {
            output.push_str(&format!("{}: ", msg.role));
            match msg.content {
                PromptContent::Text { text } => {
                    output.push_str(&text);
                }
                PromptContent::Image { .. } => {
                    output.push_str("[Image Content]");
                }
                PromptContent::Resource { .. } => {
                    output.push_str("[Embedded Resource]");
                }
            }
            output.push('\n');
        }

        Ok((output, summary))
    }
}

pub struct McpListPromptsTool;

#[async_trait]
impl Tool for McpListPromptsTool {
    fn name(&self) -> &str {
        "mcp_list_prompts"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::api::types::FunctionDefinition {
                name: self.name().to_string(),
                description: "List available conversation templates (prompts) from an MCP server."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "The name of the MCP server to list prompts from."
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
        let server_name = args["server_name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing server_name"))?;

        let summary = format!("Listing MCP prompts from {}", server_name);
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let config = context
            .config
            .mcp_servers
            .iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| anyhow!("MCP server {} not found", server_name))?;

        let client_mutex = context
            .mcp_manager
            .get_client(config, Some(context.task_manager.clone()))
            .await?;
        let mut client = client_mutex.lock().await;
        let prompts = client.list_prompts().await?;

        let mut output = format!("Prompts from {}:\n", server_name);
        for p in prompts {
            output.push_str(&format!(
                "- {}: {}\n",
                p.name,
                p.description.unwrap_or_default()
            ));
            if !p.arguments.is_empty() {
                output.push_str("  Arguments:\n");
                for arg in p.arguments {
                    output.push_str(&format!(
                        "    - {} ({}): {}\n",
                        arg.name,
                        if arg.required { "required" } else { "optional" },
                        arg.description.unwrap_or_default()
                    ));
                }
            }
        }

        Ok((output, summary))
    }
}
