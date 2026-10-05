use serde_json::{Value, json};

use crate::api::client::ApiClient;
use crate::api::types::{ToolDefinition, Usage};

/// The restricted toolset exposed to the local worker model.
pub const TOOL_READ_FILE: &str = "read_file";
pub const TOOL_WRITE_FILE: &str = "write_file";
pub const TOOL_EDIT_FILE: &str = "edit_file";
pub const TOOL_RUN_COMMAND: &str = "run_command";
pub const TOOL_FINISH_STEP: &str = "finish_step";

pub const WORKER_TOOLS: [&str; 5] = [
    TOOL_READ_FILE,
    TOOL_WRITE_FILE,
    TOOL_EDIT_FILE,
    TOOL_RUN_COMMAND,
    TOOL_FINISH_STEP,
];

/// One proposed action from the worker (tool name + parsed arguments).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerAction {
    pub id: String,
    pub tool: String,
    pub args: Value,
}

/// Result of one worker invocation.
#[derive(Clone, Debug)]
pub struct WorkerTurn {
    pub actions: Vec<WorkerAction>,
    pub content: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("worker API error: {0}")]
    Api(#[from] crate::errors::ApiError),
    #[error("worker proposed unknown tool '{0}'")]
    UnknownTool(String),
    #[error("worker produced no usable action: {0}")]
    NoAction(String),
}

/// Everything the worker sees for one attempt. Built fresh every time —
/// the worker never sees conversation history.
#[derive(Clone, Debug, Default)]
pub struct WorkerPrompt {
    pub goal: String,
    pub step_id: String,
    pub step_description: String,
    pub invariants: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub attempt: u32,
    pub max_attempts: u32,
    /// e.g. verification failure output, rejected-action reasons, escalation guidance
    pub context: Vec<String>,
}

pub struct Worker {
    client: ApiClient,
    model: String,
    temperature: f64,
    max_tokens: u32,
    reasoning_effort: Option<String>,
}

impl Worker {
    pub fn new(
        client: ApiClient,
        model: String,
        temperature: f64,
        max_tokens: u32,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            client,
            model,
            temperature,
            max_tokens,
            reasoning_effort,
        }
    }

    pub fn tool_definitions() -> Vec<ToolDefinition> {
        fn def(name: &str, description: &str, props: Value, required: &[&str]) -> ToolDefinition {
            ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::api::types::FunctionDefinition {
                    name: name.to_string(),
                    description: description.to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": props,
                        "required": required,
                    }),
                },
            }
        }
        vec![
            def(
                TOOL_READ_FILE,
                "Read a file inside the workspace. Returns its full contents.",
                json!({ "path": { "type": "string", "description": "workspace-relative path" } }),
                &["path"],
            ),
            def(
                TOOL_WRITE_FILE,
                "Create or fully overwrite a file with the given contents.",
                json!({
                    "path": { "type": "string" },
                    "content": { "type": "string", "description": "complete new file contents" }
                }),
                &["path", "content"],
            ),
            def(
                TOOL_EDIT_FILE,
                "Replace the first exact occurrence of old_string with new_string in a file.",
                json!({
                    "path": { "type": "string" },
                    "old_string": { "type": "string", "description": "exact existing text, unique enough to match once" },
                    "new_string": { "type": "string" }
                }),
                &["path", "old_string", "new_string"],
            ),
            def(
                TOOL_RUN_COMMAND,
                "Run a deterministic shell command in the workspace (build, test, lint). No interactive commands.",
                json!({ "command": { "type": "string" } }),
                &["command"],
            ),
            def(
                TOOL_FINISH_STEP,
                "Declare the step complete. Only call when every invariant holds.",
                json!({ "summary": { "type": "string", "description": "one line: what you changed and why invariants hold" } }),
                &["summary"],
            ),
        ]
    }

    /// One bounded-context proposal. Returns parsed actions; performs at
    /// most one repair round when the model emits invalid tool arguments.
    pub async fn propose(&self, prompt: &WorkerPrompt) -> Result<WorkerTurn, WorkerError> {
        let messages = build_messages(prompt, None);
        let turn = self.complete(&messages).await?;
        match self.parse_actions(turn) {
            Ok(actions) => Ok(actions),
            Err(first) => {
                let repair_reason = first.to_string();
                let messages = build_messages(prompt, Some(&repair_reason));
                let turn = self.complete(&messages).await?;
                self.parse_actions(turn)
            }
        }
    }

    async fn complete(
        &self,
        messages: &[crate::api::types::ChatMessage],
    ) -> Result<crate::api::client::CompletionOutcome, WorkerError> {
        Ok(self
            .client
            .chat_completion_with_usage(
                messages.to_vec(),
                &self.model,
                Some(self.temperature),
                Some(self.max_tokens),
                Some(Self::tool_definitions()),
                self.reasoning_effort.as_deref(),
            )
            .await?)
    }

    fn parse_actions(
        &self,
        outcome: crate::api::client::CompletionOutcome,
    ) -> Result<WorkerTurn, WorkerError> {
        let mut actions = Vec::new();
        for call in &outcome.tool_calls {
            let name = call.function.name.clone();
            if !WORKER_TOOLS.contains(&name.as_str()) {
                return Err(WorkerError::UnknownTool(name));
            }
            let args: Value = if call.function.arguments.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&call.function.arguments).map_err(|e| WorkerError::NoAction(format!(
                    "tool '{name}' arguments are not valid JSON ({e}): {}",
                    truncate(&call.function.arguments)
                )))?
            };
            actions.push(WorkerAction {
                id: call.id.clone(),
                tool: name,
                args,
            });
        }
        if actions.is_empty() {
            return Err(WorkerError::NoAction(format!(
                "no tool call in reply: {}",
                truncate(&outcome.content)
            )));
        }
        Ok(WorkerTurn {
            actions,
            content: outcome.content,
            usage: outcome.usage,
        })
    }
}

fn truncate(s: &str) -> &str {
    if s.len() > 500 {
        let mut end = 500;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    } else {
        s
    }
}

fn build_messages(
    prompt: &WorkerPrompt,
    repair: Option<&str>,
) -> Vec<crate::api::types::ChatMessage> {
    let mut system = String::from(
        "You are an autonomous coding worker executing ONE step of a larger plan, \
under a governor that reviews every action before it runs.\n\
Hard rules:\n\
1. Modify ONLY files under the step's allowed paths. Reads outside them are allowed when needed for context.\n\
2. File paths are ALWAYS relative to the workspace root (e.g. 'src/lib.rs'). NEVER use absolute paths.\n\
3. Make the smallest change that satisfies the step invariants. Do not refactor, rename, or 'improve' anything else.\n\
4. Use read_file before editing a file you have not seen in this context.\n\
5. run_command is for deterministic checks only (build/test/lint/inspect). Never run interactive, network-installing, or destructive commands.\n\
6. Call finish_step exactly when you believe every invariant holds. Its summary must state how each invariant is satisfied.\n\
7. Every reply must contain exactly one tool call.\n",
    );
    if let Some(reason) = repair {
        system.push_str(&format!(
            "\nYour previous reply was invalid and was NOT executed: {reason}\nReply again with exactly one valid tool call.\n"
        ));
    }

    let mut user = String::new();
    user.push_str(&format!("PROJECT GOAL: {}\n", prompt.goal));
    user.push_str(&format!(
        "\nCURRENT STEP [{}]: {}\n",
        prompt.step_id, prompt.step_description
    ));
    user.push_str("INVARIANTS (all must hold when you finish):\n");
    for inv in &prompt.invariants {
        user.push_str(&format!("- {inv}\n"));
    }
    if prompt.allowed_paths.is_empty() {
        user.push_str("\nALLOWED PATHS: none — this step is read-only; writing is denied.\n");
    } else {
        user.push_str("\nALLOWED PATHS (writes permitted only beneath these):\n");
        for p in &prompt.allowed_paths {
            user.push_str(&format!("- {p}\n"));
        }
    }
    user.push_str(&format!(
        "\nATTEMPT {}/{} for this step.\n",
        prompt.attempt, prompt.max_attempts
    ));
    for (i, ctx) in prompt.context.iter().enumerate() {
        user.push_str(&format!("\nCONTEXT {i}:\n{ctx}\n"));
    }
    user.push_str("\nReply with exactly one tool call.");

    vec![
        crate::api::types::ChatMessage::system(&system),
        crate::api::types::ChatMessage::user(&user),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_are_wellformed() {
        let defs = Worker::tool_definitions();
        assert_eq!(defs.len(), 5);
        assert!(defs.iter().all(|d| d.function.parameters["type"] == "object"));
    }

    #[test]
    fn prompt_lists_invariants_and_allowlist() {
        let p = WorkerPrompt {
            goal: "ship it".into(),
            step_id: "s1".into(),
            step_description: "add tests".into(),
            invariants: vec!["tests pass".into()],
            allowed_paths: vec!["tests/".into()],
            attempt: 2,
            max_attempts: 3,
            context: vec!["last error: E0432".into()],
        };
        let msgs = build_messages(&p, Some("bad json"));
        let user = msgs[1].content.as_deref().unwrap();
        assert!(user.contains("INVARIANTS"));
        assert!(user.contains("tests pass"));
        assert!(user.contains("tests/"));
        assert!(user.contains("ATTEMPT 2/3"));
        assert!(user.contains("CONTEXT 0"));
        assert!(msgs[0].content.as_deref().unwrap().contains("bad json"));
    }

    #[test]
    fn read_only_step_warns_worker() {
        let p = WorkerPrompt {
            allowed_paths: vec![],
            ..Default::default()
        };
        let msgs = build_messages(&p, None);
        assert!(msgs[1].content.as_deref().unwrap().contains("read-only"));
    }
}
