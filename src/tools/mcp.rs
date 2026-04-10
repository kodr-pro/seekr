use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::mcp::types::McpToolDefinition;
use crate::tools::{ExecutionContext, Tool};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

pub struct McpTool {
    pub server_name: String,
    pub definition: ToolDefinition,
}

impl McpTool {
    pub fn new(server_name: String, mcp_def: McpToolDefinition) -> Self {
        Self {
            server_name,
            definition: ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: mcp_def.name,
                    description: mcp_def.description,
                    parameters: mcp_def.input_schema,
                },
            },
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.definition.function.name
    }

    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(
        &self,
        args: &Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let summary = format!("mcp_tool {}: {}", self.server_name, self.name());
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        // Find the server config
        let server_config = context
            .config
            .mcp_servers
            .iter()
            .find(|s| s.name == self.server_name)
            .ok_or_else(|| anyhow!("MCP server {} configuration not found", self.server_name))?;

        let client_mutex = context
            .mcp_manager
            .get_client(server_config, Some(context.task_manager.clone()))
            .await?;
        let mut client = client_mutex.lock().await;

        let result = client.call_tool(self.name(), args.clone()).await?;

        let mut output = String::new();
        for content in result.content {
            match content {
                crate::mcp::types::McpContent::Text { text } => output.push_str(&text),
                crate::mcp::types::McpContent::Image { .. } => {
                    output.push_str("\n[Image content received, but not displayed in TUI]")
                }
                crate::mcp::types::McpContent::Resource { resource } => {
                    output.push_str(&format!("\n[Resource content: {:?}]", resource))
                }
            }
        }

        if result.is_error {
            Err(anyhow!(output))
        } else {
            Ok((output, format!("Executed MCP tool {}", self.name())))
        }
    }
}
