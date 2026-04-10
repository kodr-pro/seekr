use crate::agent::system_prompt::AgentRole;
use crate::agent::{AgentCommand, AgentEvent, AgentLoop};
use crate::api::types::ToolDefinition;
use crate::tools::{Tool, ExecutionContext};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;

pub struct SubAgentTool;

impl SubAgentTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &str {
        "call_subagent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::api::types::FunctionDefinition {
                name: self.name().to_string(),
                description: "Delegates a specific, well-defined task to a specialized sub-agent. This is useful for planning, research, or complex code analysis without cluttering the main context. Returns the sub-agent's final summary.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "role": {
                            "type": "string",
                            "enum": ["planner", "explorer"],
                            "description": "The specialized role of the sub-agent."
                        },
                        "task": {
                            "type": "string",
                            "description": "The specific instruction or request for the sub-agent."
                        }
                    },
                    "required": ["role", "task"]
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
        let role_str = args["role"].as_str().ok_or_else(|| anyhow!("Missing role"))?;
        let task = args["task"].as_str().ok_or_else(|| anyhow!("Missing task"))?;

        let role = match role_str {
            "planner" => AgentRole::Planner,
            "explorer" => AgentRole::Explorer,
            _ => AgentRole::Main,
        };

        let summary = format!("Sub-agent ({}) started: {}", role_str, crate::tools::truncate(task, 40));
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        // Setup sub-agent communication
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let cmd_tx_clone = command_tx.clone();

        let sub_agent = AgentLoop::new(
            context.config.clone(),
            event_tx,
            command_rx,
            cmd_tx_clone,
            context.registry.clone(),
            role,
            context.mcp_manager.clone(),
        );

        // Inject the task as a user message
        command_tx.send(AgentCommand::UserMessage(task.to_string()))?;

        // Run the sub-agent in a separate task
        tokio::spawn(async move {
            sub_agent.run().await;
        });

        // Collect events until completion
        let mut final_answer = String::new();
        let mut tool_results = Vec::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::ContentDelta(text) => {
                    final_answer.push_str(&text);
                }
                AgentEvent::ToolCallResult { name, result } => {
                   tool_results.push(format!("{}: {}", name, crate::tools::truncate(&result, 30)));
                }
                AgentEvent::TurnComplete => {
                   // For sub-agents, we usually want them to finish in one turn or stop when they give a final answer.
                   // However, AgentLoop continues until it stops calling tools.
                   // We'll wait for the process to actually finish (AgentLoop::run returns on Shutdown or completion).
                }
                AgentEvent::MaxIterationsReached => {
                    break;
                }
                AgentEvent::Error(e) => {
                    return Err(anyhow!("Sub-agent error: {}", e));
                }
                AgentEvent::Activity(activity) => {
                    // Bubble up activity to parent task manager
                    context.task_manager.log_activity(
                        &format!("sub_{}_{}", role_str, activity.tool_name),
                        &activity.summary,
                        activity.status,
                        thread_id,
                        total_threads,
                    );
                }
                _ => {}
            }
        }

        Ok((
            final_answer,
            format!("Sub-agent ({}) finished.", role_str)
        ))
    }
}
