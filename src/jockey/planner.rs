use anyhow::Result;
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

use crate::api::client::{ApiClient, CompletionOutcome};
use crate::api::types::Usage;
use crate::jockey::dag::{DagError, TaskDag};

use super::dag::StepStatus;

#[derive(Clone, Debug, Deserialize)]
pub struct Clarification {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub why: String,
}

#[derive(Clone, Debug)]
pub enum PlanOutcome {
    NeedsClarification(Vec<Clarification>),
    Ready(Box<TaskDag>),
}

#[derive(Clone, Debug)]
pub struct PlanResponse {
    pub outcome: PlanOutcome,
    pub usage: Option<Usage>,
}

#[derive(Debug, Error)]
pub enum PlannerError {
    #[error("frontier API error: {0}")]
    Api(#[from] crate::errors::ApiError),
    #[error("frontier returned unparseable JSON: {0}")]
    Parse(String),
    #[error("frontier produced an invalid DAG: {0}")]
    InvalidDag(#[from] DagError),
    #[error("frontier returned no questions and no DAG")]
    Empty,
}

#[derive(Deserialize)]
struct FrontierReply {
    needs_clarification: bool,
    #[serde(default)]
    questions: Vec<Clarification>,
    #[serde(default)]
    dag: Option<RawDag>,
}

#[derive(Deserialize)]
struct RawDag {
    goal: String,
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
struct RawStep {
    id: String,
    description: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    invariants: Vec<String>,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    verification_command: Option<String>,
}

/// Phase 0 planner: turns a raw goal (plus clarification answers) into a
/// validated `TaskDag`, or asks for clarification first. Bounded to one
/// repair round on malformed output.
pub struct FrontierPlanner {
    client: ApiClient,
    model: String,
    temperature: f64,
    max_tokens: u32,
}

impl FrontierPlanner {
    pub fn new(client: ApiClient, model: String, temperature: f64, max_tokens: u32) -> Self {
        Self {
            client,
            model,
            temperature,
            max_tokens,
        }
    }

    pub async fn plan(
        &self,
        goal: &str,
        clarifications: &[(String, String)],
        repo_tree: &str,
    ) -> Result<PlanResponse, PlannerError> {
        let messages = self.build_messages(goal, clarifications, repo_tree, None);
        let outcome = self.complete_and_parse(&messages, goal).await;
        match outcome {
            Ok(response) => Ok(response),
            Err(first) => {
                let repair_note = format!(
                    "Your previous reply was rejected: {first}. Reply again with ONLY the JSON object, no markdown fences, no commentary."
                );
                let messages = self.build_messages(goal, clarifications, repo_tree, Some(&repair_note));
                self.complete_and_parse(&messages, goal).await
            }
        }
    }

    fn build_messages(
        &self,
        goal: &str,
        clarifications: &[(String, String)],
        repo_tree: &str,
        repair_note: Option<&str>,
    ) -> Vec<crate::api::types::ChatMessage> {
        let mut system = String::from(
            "You are the planning phase of an autonomous coding agent. You never write code; \
you decompose goals into a rigid task DAG executed by a small local model.\n\
Reply with ONLY a JSON object, no markdown fences. Two legal shapes:\n\
{\"needs_clarification\": true, \"questions\": [{\"id\": \"q1\", \"question\": \"...\", \"why\": \"...\"}]}\n\
{\"needs_clarification\": false, \"dag\": {\"goal\": \"...\", \"steps\": [...]}}\n\
Rules:\n\
- Ask for clarification (max 3 questions) ONLY if the goal is ambiguous about behavior, acceptance criteria, or file boundaries. Otherwise produce the DAG.\n\
- Each step: {\"id\": \"snake_case\", \"description\": \"imperative one-liner\", \"depends_on\": [ids], \"invariants\": [\"must hold after the step\"], \"allowed_paths\": [\"relative paths or dirs this step may touch\"], \"verification_command\": \"deterministic exit-code command or null\"}\n\
- invariants: 1-3 concrete, checkable statements. Never empty.\n\
- allowed_paths: paths the step may read/write, relative to the repo root. Use the exact paths from the repository tree. Empty array = read-only step.\n\
- verification_command must be deterministic and exit-code based (e.g. \"cargo test -p foo\", \"node -e ...\", \"python -m pytest tests/x\"). Never a command that prompts, watches, or serves.\n\
- depends_on must reference earlier steps only; no cycles.\n\
- Order steps so each is independently verifiable. Prefer 2-8 steps total.\n\
- Stay inside the repository. Never plan network-auth changes, secret access, or destructive git operations.\n",
        );
        if !repo_tree.trim().is_empty() {
            system.push_str(&format!("\nRepository file tree (root-relative):\n{repo_tree}\n"));
        }
        if let Some(note) = repair_note {
            system.push_str(&format!("\nREPAIR: {note}\n"));
        }

        let mut user = format!("GOAL: {goal}\n");
        for (id, answer) in clarifications {
            user.push_str(&format!("ANSWER to {id}: {answer}\n"));
        }
        user.push_str("\nProduce the JSON object now.");

        vec![
            crate::api::types::ChatMessage::system(&system),
            crate::api::types::ChatMessage::user(&user),
        ]
    }

    async fn complete_and_parse(
        &self,
        messages: &[crate::api::types::ChatMessage],
        fallback_goal: &str,
    ) -> Result<PlanResponse, PlannerError> {
        let CompletionOutcome { content, usage, .. } = self
            .client
            .chat_completion_with_usage(
                messages.to_vec(),
                &self.model,
                Some(self.temperature),
                Some(self.max_tokens),
                None,
                None,
            )
            .await?;
        let reply: FrontierReply = parse_json_object(&content)
            .ok_or_else(|| PlannerError::Parse("no JSON object found in reply".into()))
            .and_then(|v| {
                serde_json::from_value(v).map_err(|e| PlannerError::Parse(e.to_string()))
            })?;

        if reply.needs_clarification {
            if reply.questions.is_empty() {
                return Err(PlannerError::Empty);
            }
            let questions: Vec<Clarification> = reply
                .questions
                .into_iter()
                .take(3)
                .filter(|q| !q.question.trim().is_empty())
                .collect();
            if questions.is_empty() {
                return Err(PlannerError::Empty);
            }
            return Ok(PlanResponse {
                outcome: PlanOutcome::NeedsClarification(questions),
                usage,
            });
        }

        let raw = reply.dag.ok_or(PlannerError::Empty)?;
        let mut dag = TaskDag {
            goal: raw.goal,
            steps: raw
                .steps
                .into_iter()
                .map(|s| crate::jockey::dag::DagStep {
                    id: s.id,
                    description: s.description,
                    depends_on: s.depends_on,
                    invariants: s.invariants,
                    allowed_paths: s.allowed_paths.into_iter().map(std::path::PathBuf::from).collect(),
                    verification_command: s.verification_command,
                    status: StepStatus::Pending,
                })
                .collect(),
        };
        if dag.goal.trim().is_empty() {
            dag.goal = fallback_goal.to_string();
        }
        dag.validate()?;
        Ok(PlanResponse {
            outcome: PlanOutcome::Ready(Box::new(dag)),
            usage,
        })
    }
}

/// Extracts the outermost JSON object from model output (tolerates prose
/// and markdown fences around it).
pub fn parse_json_object(content: &str) -> Option<serde_json::Value> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&content[start..=end]).ok()
}

/// Compact repository tree for planner context. Skips VCS/build/vendor dirs.
pub fn collect_repo_tree(root: &Path, max_entries: usize) -> String {
    fn walk(dir: &Path, root: &Path, depth: usize, out: &mut Vec<String>, budget: &mut usize) {
        if depth > 6 || *budget == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<_> = entries.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for entry in items {
            if *budget == 0 {
                return;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "node_modules" | "dist" | "build" | "vendor" | "__pycache__"
                )
            {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if path.is_dir() {
                out.push(format!("{}/", rel.display()));
                *budget -= 1;
                walk(&path, root, depth + 1, out, budget);
            } else {
                out.push(rel.display().to_string());
                *budget -= 1;
            }
        }
    }

    let mut out = Vec::new();
    let mut budget = max_entries;
    walk(root, root, 0, &mut out, &mut budget);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_json_from_fenced_output() {
        let raw = "Here is the plan:\n```json\n{\"needs_clarification\": false, \"dag\": {\"goal\": \"g\", \"steps\": []}}\n```\nthanks";
        let v = parse_json_object(raw).unwrap();
        assert_eq!(v["dag"]["goal"], "g");
    }

    #[test]
    fn parse_json_rejects_garbage() {
        assert!(parse_json_object("no json here").is_none());
    }

    #[test]
    fn frontier_reply_parses_clarification_shape() {
        let v = json!({
            "needs_clarification": true,
            "questions": [
                {"id": "q1", "question": "Which crate?", "why": "two crates exist"}
            ]
        });
        let reply: FrontierReply = serde_json::from_value(v).unwrap();
        assert!(reply.needs_clarification);
        assert_eq!(reply.questions.len(), 1);
    }

    #[test]
    fn repo_tree_skips_noise_and_stays_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::create_dir_all(tmp.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("target/debug/artifact"), "").unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();

        let tree = collect_repo_tree(tmp.path(), 400);
        assert!(tree.contains("src/main.rs"));
        assert!(tree.contains("Cargo.toml"));
        assert!(!tree.contains("target"));
        assert!(!tree.contains(".git"));

        let bounded = collect_repo_tree(tmp.path(), 1);
        assert!(bounded.lines().count() <= 1);
    }
}
