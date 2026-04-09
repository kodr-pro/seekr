use crate::api::client::ApiClient;
use crate::api::types::{ChatMessage, FunctionDefinition, ToolDefinition};
use crate::tools::task::TaskManager;
use crate::tools::{ActivityStatus, Tool};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::json;

pub struct SubmitForPeerReviewTool;

#[async_trait]
impl Tool for SubmitForPeerReviewTool {
    fn name(&self) -> &str {
        "submit_for_peer_review"
    } // name

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Submit your work for an AI Peer Review. MUST be called when you believe the OVERARCHING user request is completely fulfilled, before you provide your final conversation response.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "overarching_objective": { "type": "string", "description": "The original main request the user asked you to solve." },
                        "summary_of_accomplishments": { "type": "string", "description": "A very detailed summary of what you implemented, files changed, and how it satisfies the objective." }
                    },
                    "required": ["overarching_objective", "summary_of_accomplishments"]
                }),
            },
        }
    } // definition

    async fn execute(
        &self,
        args: &serde_json::Value,
        task_manager: &TaskManager,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let objective = args["overarching_objective"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing overarching_objective"))?;
        let summary = args["summary_of_accomplishments"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing summary_of_accomplishments"))?;

        let config = match task_manager.config.as_ref() {
            Some(c) => c,
            None => {
                return Ok((
                    "Error: Missing configuration for peer review API client.".to_string(),
                    "Missing API config".to_string(),
                ));
            }
        };

        if !config.agent.enable_peer_review {
            task_manager.log_activity(
                self.name(),
                "Peer review is disabled in settings by the user. Proceeding automatically.",
                ActivityStatus::Success,
                thread_id,
                total_threads,
            );
            return Ok((
                "Peer Review Disabled. You may now provide your final response to the user."
                    .to_string(),
                "Peer review disabled (auto-passed)".to_string(),
            ));
        }

        task_manager.log_activity(
            self.name(),
            "Initiating AI Peer Review background check...",
            ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let activities = task_manager.activities();
        let mut text_activities = String::new();
        for a in activities.iter().rev().take(30).rev() {
            text_activities.push_str(&format!("- [{}] {}\n", a.tool_name, a.summary));
        }

        let prompt = format!(
            "You are an AI Peer Reviewer. Another agent claims to have completed the following overarching objective:\n\n\
            OBJECTIVE: {}\n\n\
            The agent provided this summary of their accomplishments:\n\
            {}\n\n\
            Here is their recent action log (latest 30 actions):\n\
            {}\n\n\
            Evaluate whether the work is truly complete and satisfies the objective. \
            If it is completely solved and no further actions are necessary, reply EXACTLY with 'APPROVED'. \
            If it is missing things, incomplete, or if you suspect hallucinations based on the action log, reply with 'REJECTED' followed immediately by a tight bulleted list of missing requirements or errors that the agent must fix.",
            objective, summary, text_activities
        );

        let client = ApiClient::new(config);

        let messages = vec![
            ChatMessage::system("You are a strict code and logic reviewer."),
            ChatMessage::user(&prompt),
        ];

        let result = match client
            .chat_completion(messages, &config.current_provider().model)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok((
                    format!(
                        "Peer Review System Error: {}. Treat as passed and provide your final response.",
                        e
                    ),
                    "Peer review API failed".to_string(),
                ));
            }
        };

        if result.trim().starts_with("APPROVED") {
            Ok((
                "Peer Review Passed. You may now provide your final response to the user."
                    .to_string(),
                "Peer review passed".to_string(),
            ))
        } else {
            Ok((
                format!(
                    "Peer Review FAILED. Please address the following issues immediately:\n{}",
                    result
                ),
                "Peer review FAILED (returned to agent)".to_string(),
            ))
        }
    } // execute
} // impl SubmitForPeerReviewTool
