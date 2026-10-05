use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::api::client::ApiClient;
use crate::config::AppConfig;
use crate::jev::JevClient;
use crate::jockey::dag::{DagStep, StepStatus, TaskDag};
use crate::jockey::interceptor::{
    AttemptRecord, InterceptVerdict, Interceptor,
};
use crate::jockey::ledger::{CostLedger, EventLog, JockeyEvent};
use crate::jockey::worker::{
    TOOL_EDIT_FILE, TOOL_FINISH_STEP, TOOL_READ_FILE, TOOL_RUN_COMMAND,
    TOOL_WRITE_FILE, Worker, WorkerAction, WorkerPrompt,
};
use crate::sandbox::git::GitSandbox;
use crate::sandbox::paths::PathSandbox;
use crate::sandbox::run_command;

const MAX_ACTIONS_PER_ATTEMPT: usize = 16;
const MAX_REJECTIONS_PER_ATTEMPT: usize = 6;
const MAX_READ_CHARS: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum GovernorError {
    #[error("sandbox error: {0}")]
    Sandbox(#[from] crate::sandbox::git::SandboxError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub goal: String,
    pub repo_root: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub dag: TaskDag,
    pub ledger: CostLedger,
    pub frontier_calls_used: u32,
    pub status: String,
}

impl RunState {
    pub fn state_dir(run_id: &str) -> Option<PathBuf> {
        dirs::state_dir()
            .or_else(dirs::data_dir)
            .map(|d| d.join("seekr").join("jockey").join(run_id))
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = Self::state_dir(&self.run_id).ok_or_else(|| {
            std::io::Error::other("cannot determine state directory")
        })?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("state.json"),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        )
    }

    pub fn load(run_id: &str) -> std::io::Result<Self> {
        let dir = Self::state_dir(run_id).ok_or_else(|| {
            std::io::Error::other("cannot determine state directory")
        })?;
        let raw = std::fs::read_to_string(dir.join("state.json"))?;
        serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn list_runs() -> Vec<(String, String)> {
        let Some(dir) = dirs::state_dir().or_else(dirs::data_dir) else {
            return vec![];
        };
        let root = dir.join("seekr").join("jockey");
        let Ok(entries) = std::fs::read_dir(root) else {
            return vec![];
        };
        let mut runs: Vec<(String, String)> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| {
                let id = e.file_name().to_string_lossy().to_string();
                let status = Self::load(&id)
                    .ok()
                    .map(|s| s.status)
                    .unwrap_or_else(|| "unknown".to_string());
                (id, status)
            })
            .collect();
        runs.sort();
        runs
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunOutcome {
    Success,
    Failed(String),
}

/// The autonomous governor: drives the DAG through the 4-tier loop
/// (worker -> interceptor -> deterministic gate -> triage/escalation).
pub struct Governor {
    pub state: RunState,
    worker: Worker,
    frontier: Option<(ApiClient, String)>,
    interceptor: Interceptor,
    sandbox: GitSandbox,
    jj: crate::config::JockeyConfig,
    shell_blocklist: Vec<String>,
    event_tx: mpsc::UnboundedSender<JockeyEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<JockeyEvent>>,
    event_log: EventLog,
    attempts: BTreeMap<String, Vec<AttemptRecord>>,
    extra_context: BTreeMap<String, Vec<String>>,
}

impl Governor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &AppConfig,
        worker: Worker,
        frontier: Option<(ApiClient, String)>,
        jev: JevClient,
        sandbox: GitSandbox,
        goal: String,
        dag: TaskDag,
        run_id: String,
    ) -> Result<Self, GovernorError> {
        let state_dir = RunState::state_dir(&run_id).ok_or_else(|| {
            GovernorError::Failed("cannot determine state directory".into())
        })?;
        let event_log = EventLog::create(&state_dir)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let state = RunState {
            run_id: run_id.clone(),
            goal,
            repo_root: sandbox.repo_root().to_path_buf(),
            worktree: sandbox.worktree().to_path_buf(),
            branch: sandbox.branch().to_string(),
            dag,
            ledger: CostLedger::default(),
            frontier_calls_used: 0,
            status: "running".to_string(),
        };
        state.save()?;
        Ok(Self {
            state,
            worker,
            frontier,
            interceptor: Interceptor::new(jev, &config.jockey),
            sandbox,
            jj: config.jockey.clone(),
            shell_blocklist: config.agent.shell_blocklist.clone(),
            event_tx,
            event_rx: Some(event_rx),
            event_log,
            attempts: BTreeMap::new(),
            extra_context: BTreeMap::new(),
        })
    }

    /// Attach to an existing run (resume). Re-creates the channel pair;
    /// history is available via the run's events.jsonl.
    pub fn attach(
        config: &AppConfig,
        worker: Worker,
        frontier: Option<(ApiClient, String)>,
        jev: JevClient,
        sandbox: GitSandbox,
        mut state: RunState,
    ) -> Result<Self, GovernorError> {
        let state_dir =
            RunState::state_dir(&state.run_id).ok_or_else(|| {
                GovernorError::Failed("cannot determine state directory".into())
            })?;
        let event_log = EventLog::create(&state_dir)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        state.status = "running".to_string();
        Ok(Self {
            state,
            worker,
            frontier,
            interceptor: Interceptor::new(jev, &config.jockey),
            sandbox,
            jj: config.jockey.clone(),
            shell_blocklist: config.agent.shell_blocklist.clone(),
            event_tx,
            event_rx: Some(event_rx),
            event_log,
            attempts: BTreeMap::new(),
            extra_context: BTreeMap::new(),
        })
    }

    pub fn take_event_rx(&mut self) -> mpsc::UnboundedReceiver<JockeyEvent> {
        self.event_rx
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1)
    }

    fn emit(&mut self, event: JockeyEvent) {
        self.event_log.append(&event);
        let _ = self.event_tx.send(event.clone());
        if let JockeyEvent::Usage { .. } = &event {
            return;
        }
        self.state.save().ok();
    }

    /// Runs every runnable step to completion or failure.
    pub async fn run_to_completion(
        &mut self,
    ) -> Result<RunOutcome, GovernorError> {
        self.emit(JockeyEvent::RunStarted {
            run_id: self.state.run_id.clone(),
            goal: self.state.goal.clone(),
        });
        self.emit(JockeyEvent::PlanReady {
            summary: self.state.dag.render_summary(),
        });

        loop {
            let ready: Vec<String> = self
                .state
                .dag
                .ready_steps()
                .iter()
                .map(|s| s.id.clone())
                .collect();
            if ready.is_empty() {
                break;
            }
            for step_id in ready {
                let outcome = self.run_step(&step_id).await?;
                match outcome {
                    StepResult::Completed => {}
                    StepResult::Failed(reason) => {
                        let _ = reason;
                    }
                }
            }
        }

        let failed: Vec<String> = self
            .state
            .dag
            .steps
            .iter()
            .filter(|s| {
                matches!(s.status, StepStatus::Failed | StepStatus::RolledBack)
            })
            .map(|s| s.id.clone())
            .collect();
        let outcome = if failed.is_empty() {
            self.state.status = "success".to_string();
            self.state.save().ok();
            self.emit(JockeyEvent::RunCompleted {
                status: "success".to_string(),
                ledger: self.state.ledger.clone(),
                branch: self.state.branch.clone(),
                worktree: self.state.worktree.clone(),
            });
            RunOutcome::Success
        } else {
            let reason = format!("steps failed: {}", failed.join(", "));
            self.state.status = format!("failed: {reason}");
            self.state.save().ok();
            self.emit(JockeyEvent::RunFailed {
                reason: reason.clone(),
                ledger: self.state.ledger.clone(),
            });
            RunOutcome::Failed(reason)
        };
        Ok(outcome)
    }

    async fn run_step(
        &mut self,
        step_id: &str,
    ) -> Result<StepResult, GovernorError> {
        if let Some(step) = self.state.dag.step_mut(step_id) {
            step.status = StepStatus::InProgress;
        }
        let (description, invariants, allowed) = {
            let step = self.state.dag.step(step_id).expect("step exists");
            (
                step.description.clone(),
                step.invariants.clone(),
                step.allowed_paths.clone(),
            )
        };
        self.state.save().ok();
        let sandbox = PathSandbox::new(&self.state.worktree, &allowed)
            .map_err(|e| {
                GovernorError::Failed(format!("path sandbox init failed: {e}"))
            })?;

        let step_timeout =
            std::time::Duration::from_secs(self.jj.step_timeout_secs.max(60));
        let result = tokio::time::timeout(
            step_timeout,
            self.attempt_loop(step_id, &description, &invariants, &sandbox),
        )
        .await;

        match result {
            Ok(inner) => {
                let completed = inner?;
                if completed {
                    let commit = self
                        .sandbox
                        .checkpoint(&format!(
                            "jj: step {step_id} verified [run {}]",
                            self.state.run_id
                        ))
                        .await?;
                    if let Some(step) = self.state.dag.step_mut(step_id) {
                        step.status = StepStatus::Completed;
                    }
                    self.state.save().ok();
                    self.emit(JockeyEvent::StepCompleted {
                        step_id: step_id.to_string(),
                        commit,
                    });
                    Ok(StepResult::Completed)
                } else {
                    if let Some(step) = self.state.dag.step_mut(step_id) {
                        step.status = StepStatus::RolledBack;
                    }
                    self.state.save().ok();
                    Ok(StepResult::Failed(step_id.to_string()))
                }
            }
            Err(_) => {
                let _ = self.sandbox.rollback().await;
                if let Some(step) = self.state.dag.step_mut(step_id) {
                    step.status = StepStatus::RolledBack;
                }
                self.state.save().ok();
                self.emit(JockeyEvent::StepFailed {
                    step_id: step_id.to_string(),
                    reason: format!(
                        "step exceeded {}s wall clock",
                        self.jj.step_timeout_secs
                    ),
                });
                Ok(StepResult::Failed(step_id.to_string()))
            }
        }
    }

    /// Returns true when the step eventually passed verification.
    async fn attempt_loop(
        &mut self,
        step_id: &str,
        description: &str,
        invariants: &[String],
        sandbox: &PathSandbox,
    ) -> Result<bool, GovernorError> {
        let mut last_error: Option<String> = None;
        let mut guidance: Option<String> = None;
        let mut guidance_used = false;
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            self.emit(JockeyEvent::StepStarted {
                step_id: step_id.to_string(),
                attempt,
            });

            let mut context: Vec<String> =
                self.extra_context.remove(step_id).unwrap_or_default();
            if let Some(err) = &last_error {
                context
                    .push(format!("LAST ATTEMPT FAILED verification:\n{err}"));
            }
            if let Some(g) = &guidance {
                context
                    .push(format!("FRONTIER GUIDANCE (authoritative):\n{g}"));
                guidance = None;
            }

            let (outcome, records) = self
                .worker_attempt(
                    step_id,
                    description,
                    invariants,
                    sandbox,
                    attempt,
                    &context,
                )
                .await;

            match outcome {
                AttemptOutcome::Verified => {
                    return Ok(true);
                }
                AttemptOutcome::VerificationFailed(output) => {
                    self.attempts
                        .entry(step_id.to_string())
                        .or_default()
                        .extend(records);
                    let to_commit = self.sandbox.current_commit().await?;
                    self.sandbox.rollback().await?;
                    self.emit(JockeyEvent::RolledBack {
                        step_id: step_id.to_string(),
                        to_commit,
                    });
                    last_error = Some(output);
                    let error_ref = last_error.clone();

                    let verdict = self
                        .triage(step_id, description, invariants, &error_ref)
                        .await;
                    if verdict == TriageVerdict::ReadContext
                        && let Some(file_ctx) =
                            self.read_context_for_error(&error_ref)
                    {
                        self.extra_context
                            .insert(step_id.to_string(), vec![file_ctx]);
                    }
                    let must_escalate = verdict == TriageVerdict::Deadlock
                        || attempt >= self.jj.max_attempts_per_step;
                    if must_escalate {
                        match self
                            .escalate(
                                step_id,
                                description,
                                invariants,
                                &error_ref,
                            )
                            .await?
                        {
                            EscalationOutcome::PatchVerified => {
                                return Ok(true);
                            }
                            EscalationOutcome::Guidance(g) => {
                                if guidance_used {
                                    self.emit(JockeyEvent::StepFailed {
                                        step_id: step_id.to_string(),
                                        reason: "frontier guidance already tried once; stopping"
                                            .to_string(),
                                    });
                                    return Ok(false);
                                }
                                guidance_used = true;
                                guidance = Some(g);
                            }
                            EscalationOutcome::Exhausted(reason) => {
                                self.emit(JockeyEvent::StepFailed {
                                    step_id: step_id.to_string(),
                                    reason,
                                });
                                return Ok(false);
                            }
                        }
                    }
                }
                AttemptOutcome::NoAction(reason) => {
                    self.attempts
                        .entry(step_id.to_string())
                        .or_default()
                        .extend(records);
                    if attempt >= self.jj.max_attempts_per_step {
                        self.emit(JockeyEvent::StepFailed {
                            step_id: step_id.to_string(),
                            reason,
                        });
                        return Ok(false);
                    }
                    last_error = Some(reason);
                }
            }
        }
    }

    async fn worker_attempt(
        &mut self,
        step_id: &str,
        description: &str,
        invariants: &[String],
        sandbox: &PathSandbox,
        attempt: u32,
        context: &[String],
    ) -> (AttemptOutcome, Vec<AttemptRecord>) {
        let mut actions_done = 0usize;
        let mut rejections = 0usize;
        let mut live_context: Vec<String> = context.to_vec();
        let mut records: Vec<AttemptRecord> = Vec::new();

        loop {
            if actions_done >= MAX_ACTIONS_PER_ATTEMPT
                || rejections >= MAX_REJECTIONS_PER_ATTEMPT
            {
                return (
                    AttemptOutcome::NoAction(format!(
                        "attempt aborted: {} actions done, {} rejections (caps {MAX_ACTIONS_PER_ATTEMPT}/{MAX_REJECTIONS_PER_ATTEMPT})",
                        actions_done, rejections
                    )),
                    records,
                );
            }

            let prompt = WorkerPrompt {
                goal: self.state.goal.clone(),
                step_id: step_id.to_string(),
                step_description: description.to_string(),
                invariants: invariants.to_vec(),
                allowed_paths: sandbox
                    .allowed()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect(),
                attempt,
                max_attempts: self.jj.max_attempts_per_step,
                context: live_context.clone(),
            };

            let turn = match self.worker.propose(&prompt).await {
                Ok(t) => t,
                Err(e) => {
                    self.state.ledger.add_worker(None);
                    return (
                        AttemptOutcome::NoAction(format!("worker error: {e}")),
                        records,
                    );
                }
            };
            self.state.ledger.add_worker(turn.usage.as_ref());
            self.emit(JockeyEvent::Usage {
                ledger: self.state.ledger.clone(),
            });

            let action =
                turn.actions.into_iter().next().unwrap_or(WorkerAction {
                    id: String::new(),
                    tool: String::new(),
                    args: json!({}),
                });
            let action = relativize_action(action, &self.state.worktree);

            if action.tool == TOOL_FINISH_STEP {
                return (self.verify(step_id).await, records);
            }

            let brief = crate::jockey::interceptor::action_brief(&action);
            self.emit(JockeyEvent::ActionProposed {
                step_id: step_id.to_string(),
                tool: action.tool.clone(),
                brief: brief.clone(),
                args: Some(action.args.clone()),
            });

            let failed =
                self.attempts.get(step_id).cloned().unwrap_or_default();
            let deterministic = self.deterministic_check(&action, sandbox);
            let step_view = self.step_view(step_id);
            let review = match (
                &action.tool,
                action
                    .args
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(crate::sandbox::exec::is_readonly_command),
            ) {
                (t, Some(true))
                    if t == TOOL_RUN_COMMAND && deterministic.is_none() =>
                {
                    crate::jockey::interceptor::Review {
                        verdict: InterceptVerdict::Approved {
                            scope_p: None,
                            novelty: None,
                        },
                        jev_usage: None,
                    }
                }
                _ => {
                    self.interceptor
                        .review(&action, &step_view, &failed, deterministic)
                        .await
                }
            };
            if let Some(usage) = &review.jev_usage {
                self.state.ledger.add_jev_usage(false, usage);
                self.emit(JockeyEvent::Usage {
                    ledger: self.state.ledger.clone(),
                });
            }

            match review.verdict {
                InterceptVerdict::Approved { scope_p, novelty } => {
                    self.emit(JockeyEvent::ActionApproved {
                        tool: action.tool.clone(),
                        scope_p,
                        novelty,
                    });
                    let is_mutating = match action.tool.as_str() {
                        TOOL_WRITE_FILE | TOOL_EDIT_FILE => true,
                        TOOL_RUN_COMMAND => action
                            .args
                            .get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| {
                                !crate::sandbox::exec::is_readonly_command(c)
                            })
                            .unwrap_or(true),
                        _ => false,
                    };
                    let (ok, result) =
                        self.execute_action(&action, sandbox).await;
                    self.emit(JockeyEvent::ActionExecuted {
                        tool: action.tool.clone(),
                        ok,
                        brief: truncate_str(&result, 200).to_string(),
                    });
                    actions_done += 1;
                    if is_mutating && ok {
                        records.push(AttemptRecord {
                            fingerprint: crate::jockey::interceptor::Interceptor::fingerprint(&action),
                            brief,
                        });
                    }
                    live_context.push(format!(
                        "ACTION {}: {} -> {}",
                        actions_done,
                        crate::jockey::interceptor::action_brief(&action),
                        truncate_str(&result, 4000)
                    ));
                }
                InterceptVerdict::Rejected { reason } => {
                    rejections += 1;
                    self.emit(JockeyEvent::ActionRejected {
                        tool: action.tool.clone(),
                        reason: reason.clone(),
                    });
                    live_context.push(format!(
                        "REJECTED ACTION {}: {} because: {}",
                        rejections,
                        crate::jockey::interceptor::action_brief(&action),
                        reason
                    ));
                }
            }
        }
    }

    fn step_view(&self, step_id: &str) -> DagStep {
        self.state.dag.step(step_id).expect("step exists").clone()
    }

    fn deterministic_check(
        &self,
        action: &WorkerAction,
        sandbox: &PathSandbox,
    ) -> Option<String> {
        match action.tool.as_str() {
            TOOL_READ_FILE => sandbox
                .resolve_for_read(action.args.get("path")?.as_str()?)
                .err()
                .map(|e| e.to_string()),
            TOOL_WRITE_FILE | TOOL_EDIT_FILE => {
                let path = action.args.get("path")?.as_str()?;
                sandbox.resolve_for_write(path).err().map(|e| e.to_string())
            }
            TOOL_RUN_COMMAND => {
                let command = action.args.get("command")?.as_str()?;
                crate::sandbox::exec::blocklist_violation(
                    command,
                    &self.shell_blocklist,
                )
                .map(|p| format!("command blocked by policy: matched '{p}'"))
            }
            _ => None,
        }
    }

    async fn execute_action(
        &self,
        action: &WorkerAction,
        sandbox: &PathSandbox,
    ) -> (bool, String) {
        match action.tool.as_str() {
            TOOL_READ_FILE => {
                let Some(path) =
                    action.args.get("path").and_then(|p| p.as_str())
                else {
                    return (false, "read_file: missing path".to_string());
                };
                match sandbox.resolve_for_read(path) {
                    Ok(resolved) => match std::fs::read_to_string(&resolved) {
                        Ok(content) => {
                            let brief = if content.len() > MAX_READ_CHARS {
                                format!(
                                    "{}\n... (truncated {} chars)",
                                    &content[..MAX_READ_CHARS],
                                    content.len()
                                )
                            } else {
                                content
                            };
                            (true, brief)
                        }
                        Err(e) => (false, format!("read failed: {e}")),
                    },
                    Err(e) => (false, e.to_string()),
                }
            }
            TOOL_WRITE_FILE => {
                let (Some(path), Some(content)) = (
                    action.args.get("path").and_then(|p| p.as_str()),
                    action.args.get("content").and_then(|c| c.as_str()),
                ) else {
                    return (
                        false,
                        "write_file: missing path/content".to_string(),
                    );
                };
                match sandbox.resolve_for_write(path) {
                    Ok(resolved) => {
                        if let Some(parent) = resolved.parent()
                            && std::fs::create_dir_all(parent).is_err()
                        {
                            return (
                                false,
                                format!("cannot create dirs for {path}"),
                            );
                        }
                        match std::fs::write(&resolved, content) {
                            Ok(()) => (
                                true,
                                format!(
                                    "wrote {path} ({} bytes)",
                                    content.len()
                                ),
                            ),
                            Err(e) => (false, format!("write failed: {e}")),
                        }
                    }
                    Err(e) => (false, e.to_string()),
                }
            }
            TOOL_EDIT_FILE => {
                let (Some(path), Some(old), Some(new)) = (
                    action.args.get("path").and_then(|p| p.as_str()),
                    action.args.get("old_string").and_then(|p| p.as_str()),
                    action.args.get("new_string").and_then(|p| p.as_str()),
                ) else {
                    return (
                        false,
                        "edit_file: missing path/old_string/new_string"
                            .to_string(),
                    );
                };
                match sandbox.resolve_for_write(path) {
                    Ok(resolved) => match std::fs::read_to_string(&resolved) {
                        Ok(content) => {
                            if !content.contains(old) {
                                (
                                    false,
                                    format!(
                                        "edit_file: old_string not found in {path}"
                                    ),
                                )
                            } else {
                                let updated = content.replacen(old, new, 1);
                                match std::fs::write(&resolved, updated) {
                                    Ok(()) => (true, format!("edited {path}")),
                                    Err(e) => {
                                        (false, format!("write failed: {e}"))
                                    }
                                }
                            }
                        }
                        Err(e) => (false, format!("read failed: {e}")),
                    },
                    Err(e) => (false, e.to_string()),
                }
            }
            TOOL_RUN_COMMAND => {
                let Some(command) =
                    action.args.get("command").and_then(|c| c.as_str())
                else {
                    return (false, "run_command: missing command".to_string());
                };
                let outcome = run_command(
                    &self.state.worktree,
                    command,
                    self.jj.verification_timeout_secs,
                    &self.shell_blocklist,
                )
                .await
                .unwrap_or_else(|e| {
                    crate::sandbox::CommandOutcome {
                        success: false,
                        exit_code: None,
                        timed_out: false,
                        stdout: String::new(),
                        stderr: format!("spawn failed: {e}"),
                    }
                });
                (outcome.success, outcome.combined())
            }
            _ => (false, format!("unknown tool {}", action.tool)),
        }
    }

    async fn verify(&mut self, step_id: &str) -> AttemptOutcome {
        let command = self
            .state
            .dag
            .step(step_id)
            .and_then(|s| s.verification_command.clone());
        let Some(command) = command else {
            return AttemptOutcome::Verified;
        };
        self.emit(JockeyEvent::VerificationStarted {
            step_id: step_id.to_string(),
            command: command.clone(),
        });
        let start = std::time::Instant::now();
        let outcome = run_command(
            &self.state.worktree,
            &command,
            self.jj.verification_timeout_secs,
            &self.shell_blocklist,
        )
        .await
        .unwrap_or_else(|e| crate::sandbox::CommandOutcome {
            success: false,
            exit_code: None,
            timed_out: false,
            stdout: String::new(),
            stderr: format!("spawn failed: {e}"),
        });

        if outcome.success {
            self.emit(JockeyEvent::VerificationPassed {
                step_id: step_id.to_string(),
                commit: String::new(),
                duration_secs: start.elapsed().as_secs_f64(),
            });
            AttemptOutcome::Verified
        } else {
            let output = outcome.combined();
            self.emit(JockeyEvent::VerificationFailed {
                step_id: step_id.to_string(),
                output: truncate_str(&output, 2000).to_string(),
            });
            AttemptOutcome::VerificationFailed(output)
        }
    }

    async fn triage(
        &mut self,
        step_id: &str,
        description: &str,
        invariants: &[String],
        error: &Option<String>,
    ) -> TriageVerdict {
        let state = json!({
            "step": { "id": step_id, "description": description, "invariants": invariants },
            "verification_failure": truncate_str(error.as_deref().unwrap_or(""), 4000),
        });
        let mut criteria = std::collections::BTreeMap::new();
        criteria.insert("syntax_fix".to_string(), json!("A concrete compiler, linter, or test error whose fix is mechanical and visible in the failure output"));
        criteria.insert("read_context".to_string(), json!("The failure implies the worker lacks necessary context about existing code (wrong API use, missing import, misunderstanding of a file)"));
        criteria.insert("deadlock".to_string(), json!("The step's approach cannot succeed without changing strategy or the plan itself; local retries will repeat the failure"));

        let set = crate::jev::QuestionSet::new().add(
            "triage",
            crate::jev::choice(
                crate::jev::instructions(
                    "Classify the single root cause of `verification_failure` for `step`.",
                    "Choose the action that best unblocks the local worker's NEXT attempt.",
                ),
                criteria,
            ),
        );
        match self.interceptor.jev_client().ask(&state, &set).await {
            Ok(result) => {
                self.state.ledger.add_jev(&result);
                if let Some(ans) = result.answers.get("triage")
                    && let Some((choice, confidence)) = ans.as_choice()
                {
                    self.emit(JockeyEvent::Triage {
                        step_id: step_id.to_string(),
                        verdict: choice.to_string(),
                        confidence,
                    });
                    return match choice {
                        "read_context" => TriageVerdict::ReadContext,
                        "deadlock" => TriageVerdict::Deadlock,
                        _ => TriageVerdict::SyntaxFix,
                    };
                }
                TriageVerdict::SyntaxFix
            }
            Err(_) => {
                if self.jj.allow_degraded {
                    TriageVerdict::SyntaxFix
                } else {
                    TriageVerdict::Deadlock
                }
            }
        }
    }

    fn read_context_for_error(&self, error: &Option<String>) -> Option<String> {
        let error = error.as_deref()?;
        for line in error.lines().take(40) {
            let line = line.trim();
            if let Some(idx) = line.find(" --> ") {
                let rest = &line[idx + 5..];
                if let Some(file_line) = rest.split_whitespace().next()
                    && let Some(file) = file_line.split(':').next()
                    && let Some(ctx) = self.try_read_worktree_file(file)
                {
                    return Some(ctx);
                }
            }
            if let Some(file) = line.strip_prefix("file:")
                && let Some(file) = file.split(':').next()
                && let Some(ctx) = self.try_read_worktree_file(file)
            {
                return Some(ctx);
            }
            for token in line.split_whitespace() {
                if token.contains('.')
                    && token.contains(':')
                    && let Some(file) = token.split(':').next()
                    && let Some(ctx) = self.try_read_worktree_file(file)
                {
                    return Some(ctx);
                }
            }
        }
        None
    }

    fn try_read_worktree_file(&self, file: &str) -> Option<String> {
        let path = Path::new(file);
        if path.is_absolute() || path.starts_with("..") {
            return None;
        }
        let full = self.state.worktree.join(path);
        if let Ok(content) = std::fs::read_to_string(&full) {
            Some(format!(
                "CONTEXT FILE {file}:\n{}",
                truncate_str(&content, 6000)
            ))
        } else {
            None
        }
    }

    /// Escalates to the frontier with a minimal payload: patch (applied and
    /// verified inline) or guidance for the next worker attempt.
    async fn escalate(
        &mut self,
        step_id: &str,
        description: &str,
        invariants: &[String],
        error: &Option<String>,
    ) -> Result<EscalationOutcome, GovernorError> {
        let Some((client, model)) = self.frontier.clone() else {
            return Ok(EscalationOutcome::Exhausted(
                "no frontier provider configured".to_string(),
            ));
        };
        if self.state.frontier_calls_used >= self.jj.max_frontier_calls {
            return Ok(EscalationOutcome::Exhausted(
                "frontier call budget exhausted".to_string(),
            ));
        }
        self.emit(JockeyEvent::Escalated {
            step_id: step_id.to_string(),
            reason: truncate_str(error.as_deref().unwrap_or("deadlock"), 300)
                .to_string(),
        });

        let step = self.step_view(step_id);
        let mut files_block = String::new();
        for path in step.allowed_paths.iter().take(2) {
            let full = self.state.worktree.join(path);
            if full.is_file() {
                if let Ok(content) = std::fs::read_to_string(&full) {
                    files_block.push_str(&format!(
                        "--- {} ---\n{}\n",
                        path.display(),
                        truncate_str(&content, 8000)
                    ));
                }
            } else if full.is_dir()
                && let Ok(entries) = std::fs::read_dir(&full)
            {
                for entry in entries.flatten().take(2) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        files_block.push_str(&format!(
                            "--- {} ---\n{}\n",
                            entry
                                .path()
                                .strip_prefix(&self.state.worktree)
                                .unwrap_or(&entry.path())
                                .display(),
                            truncate_str(&content, 8000)
                        ));
                    }
                }
            }
            if files_block.len() > 20_000 {
                break;
            }
        }

        let system = "You are the escalation tier of an autonomous coding agent. A small local \
model is stuck on ONE step. You get the step, its invariants, the relevant files, and the exact \
failure. Reply with EITHER:\n\
(1) a unified diff in a ```diff fence (paths relative to repo root) that fixes the step, OR\n\
(2) a paragraph starting with GUIDANCE: giving the local model concrete, actionable instructions.\n\
Never restate the problem; fix or direct. Keep it minimal.\n";
        let user = format!(
            "STEP [{step_id}]: {description}\nINVARIANTS:\n{}\n\nFILES:\n{files_block}\n\nFAILURE:\n{}",
            invariants
                .iter()
                .map(|i| format!("- {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            truncate_str(error.as_deref().unwrap_or("(none)"), 4000)
        );

        let outcome = client
            .chat_completion_with_usage(
                vec![
                    crate::api::types::ChatMessage::system(system),
                    crate::api::types::ChatMessage::user(&user),
                ],
                &model,
                Some(0.2),
                Some(4096),
                None,
                None,
            )
            .await;
        self.state.frontier_calls_used += 1;
        match outcome {
            Ok(completion) => {
                self.state.ledger.add_frontier(completion.usage.as_ref());
                self.emit(JockeyEvent::Usage {
                    ledger: self.state.ledger.clone(),
                });
                let content = completion.content;
                if let Some(patch) = extract_diff_fence(&content) {
                    match self.sandbox.apply_patch(&patch).await {
                        Ok(()) => {
                            if matches!(
                                self.verify(step_id).await,
                                AttemptOutcome::Verified
                            ) {
                                return Ok(EscalationOutcome::PatchVerified);
                            }
                            let _ = self.sandbox.rollback().await;
                            return Ok(EscalationOutcome::Exhausted(
                                "frontier patch applied but failed verification".to_string(),
                            ));
                        }
                        Err(e) => {
                            return Ok(EscalationOutcome::Exhausted(format!(
                                "frontier patch failed to apply: {e}"
                            )));
                        }
                    }
                }
                let guidance = content
                    .strip_prefix("GUIDANCE:")
                    .unwrap_or(&content)
                    .trim()
                    .to_string();
                self.emit(JockeyEvent::FrontierGuidance {
                    step_id: step_id.to_string(),
                    brief: truncate_str(&guidance, 300).to_string(),
                });
                Ok(EscalationOutcome::Guidance(guidance))
            }
            Err(e) => Ok(EscalationOutcome::Exhausted(format!(
                "frontier call failed: {e}"
            ))),
        }
    }
}

enum StepResult {
    Completed,
    Failed(String),
}

enum AttemptOutcome {
    Verified,
    VerificationFailed(String),
    NoAction(String),
}

enum EscalationOutcome {
    PatchVerified,
    Guidance(String),
    Exhausted(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriageVerdict {
    SyntaxFix,
    ReadContext,
    Deadlock,
}

/// Rewrites absolute worktree-rooted paths in action arguments to
/// workspace-relative form, keeping fingerprints and Jev state canonical.
fn relativize_action(mut action: WorkerAction, root: &Path) -> WorkerAction {
    let prefix = format!("{}/", root.display());
    let relative = action
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .and_then(|p| p.strip_prefix(&prefix))
        .map(|s| s.to_string());
    if let (Some(stripped), Some(obj)) = (relative, action.args.as_object_mut())
    {
        obj.insert("path".to_string(), json!(stripped));
    }
    action
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn extract_diff_fence(content: &str) -> Option<String> {
    let start = content.find("```diff")?;
    let after = &content[start + 7..];
    let end = after.find("```")?;
    Some(after[..end].trim().to_string())
}
