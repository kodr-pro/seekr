use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::api::client::ApiClient;
use crate::config::AppConfig;
use crate::jockey::dag::{DagStep, StepStatus, TaskDag};
use crate::jockey::interceptor::{self, AttemptRecord, InterceptVerdict};
use crate::jockey::ledger::{CostLedger, EventLog, JockeyEvent};
use crate::jockey::worker::{
    TOOL_EDIT_FILE, TOOL_FINISH_STEP, TOOL_READ_FILE, TOOL_RUN_COMMAND,
    TOOL_WRITE_FILE, Worker, WorkerAction, WorkerPrompt,
};
use crate::sandbox::git::GitSandbox;
use crate::sandbox::paths::PathSandbox;
use crate::sandbox::run_command;
use crate::wrangler::{
    CheckAction, PlanAction, WranglerClient, ensure_gate_config,
};

const MAX_ACTIONS_PER_ATTEMPT: usize = 16;
const MAX_REJECTIONS_PER_ATTEMPT: usize = 6;

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
        if let Ok(custom) = std::env::var("SEEKR_STATE_DIR")
            && !custom.trim().is_empty()
        {
            return Some(PathBuf::from(custom).join("jockey").join(run_id));
        }
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

/// The autonomous governor: drives the DAG through the loop
/// (worker -> deterministic interceptor -> verification + AiWrangler
/// gates -> triage/escalation).
pub struct Governor {
    pub state: RunState,
    worker: Worker,
    frontier: Option<(ApiClient, String)>,
    wrangler: WranglerClient,
    ground_truth: Option<String>,
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
        // The worktree must have a discoverable AiWrangler config before
        // any gate runs; generated ones keep their ledger in the state dir.
        let origin = ensure_gate_config(sandbox.worktree(), &state_dir)?;
        let wrangler = WranglerClient::new(
            config
                .jockey
                .wrangler_command
                .as_deref()
                .unwrap_or(crate::config::default_wrangler_command()),
            sandbox.worktree(),
            std::time::Duration::from_secs(config.jockey.wrangler_timeout_secs),
        );
        let _ = origin;
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
            wrangler,
            ground_truth: None,
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
        sandbox: GitSandbox,
        mut state: RunState,
    ) -> Result<Self, GovernorError> {
        let state_dir =
            RunState::state_dir(&state.run_id).ok_or_else(|| {
                GovernorError::Failed("cannot determine state directory".into())
            })?;
        let event_log = EventLog::create(&state_dir)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let _ = ensure_gate_config(&state.worktree, &state_dir)?;
        let wrangler = WranglerClient::new(
            config
                .jockey
                .wrangler_command
                .as_deref()
                .unwrap_or(crate::config::default_wrangler_command()),
            &state.worktree,
            std::time::Duration::from_secs(config.jockey.wrangler_timeout_secs),
        );
        state.status = "running".to_string();
        Ok(Self {
            state,
            worker,
            frontier,
            wrangler,
            ground_truth: None,
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

        // Phase 0 gate: AiWrangler judges the goal+DAG against ground
        // truth before any code is written.
        if let Some(outcome) = self.plan_gate().await? {
            return Ok(outcome);
        }

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

    /// Pre-flight AiWrangler plan gate. Returns `Some(outcome)` when the
    /// run must stop before any work, `None` to proceed. On Pass, the
    /// report's ground-truth context pack is captured for every worker
    /// prompt.
    async fn plan_gate(&mut self) -> Result<Option<RunOutcome>, GovernorError> {
        let plan =
            serde_json::to_string(&self.state.dag.steps).unwrap_or_default();
        let verdict = self
            .wrangler
            .plan(&self.state.goal, Some(truncate_str(&plan, 6000)))
            .await;
        self.state.ledger.add_wrangler(None);
        self.emit(JockeyEvent::WranglerGate {
            gate: "plan".to_string(),
            action: format!("{:?}", verdict.action),
            brief: truncate_str(verdict.raw.lines().next().unwrap_or(""), 200)
                .to_string(),
        });

        match verdict.action {
            PlanAction::Pass => {
                // Keep the context pack (ground truth the worker must
                // honor); drop the verdict lines to stay bounded.
                let pack = verdict
                    .raw
                    .split_once("context pack:")
                    .map(|(_, p)| p.trim().to_string())
                    .filter(|p| !p.is_empty());
                self.ground_truth =
                    pack.map(|p| truncate_str(&p, 8000).to_string());
                Ok(None)
            }
            PlanAction::Clarify => Ok(Some(self.stop_run(
                "needs_input",
                format!(
                    "aiwrangler plan gate: CLARIFY — resolve before coding:\n{}",
                    truncate_str(&verdict.raw, 2000)
                ),
            ))),
            PlanAction::Human => Ok(Some(self.stop_run(
                "needs_owner",
                format!(
                    "aiwrangler plan gate: HUMAN — owner decision required:\n{}",
                    truncate_str(&verdict.raw, 2000)
                ),
            ))),
            PlanAction::Inactive | PlanAction::Unreachable => {
                if self.jj.allow_degraded {
                    self.emit(JockeyEvent::WranglerGate {
                        gate: "plan".to_string(),
                        action: "degraded-pass".to_string(),
                        brief: "gate unavailable; allow_degraded is set, \
continuing without ground truth"
                            .to_string(),
                    });
                    Ok(None)
                } else {
                    Ok(Some(self.stop_run(
                        "failed",
                        format!(
                            "aiwrangler plan gate unavailable (fail-closed); \
set --allow-degraded to run without it:\n{}",
                            truncate_str(&verdict.raw, 1000)
                        ),
                    )))
                }
            }
        }
    }

    /// Marks the run stopped with a terminal status and returns the
    /// failure outcome.
    fn stop_run(&mut self, status: &str, reason: String) -> RunOutcome {
        self.state.status = status.to_string();
        self.state.save().ok();
        self.emit(JockeyEvent::RunFailed {
            reason: reason.clone(),
            ledger: self.state.ledger.clone(),
        });
        RunOutcome::Failed(reason)
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
            if let Some(pack) = &self.ground_truth {
                context.push(format!(
                    "GROUND TRUTH (AiWrangler context pack — honor while \
implementing):\n{pack}"
                ));
            }
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
                AttemptOutcome::NeedsOwner(reason) => {
                    self.emit(JockeyEvent::StepFailed {
                        step_id: step_id.to_string(),
                        reason: format!(
                            "{reason}\nOwner action required: review the \
findings above, then `seekr ack {}` (or re-run with a refined goal).",
                            self.state.run_id
                        ),
                    });
                    return Ok(false);
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
        let mut todos: Vec<(String, String)> = Vec::new();
        let mut todo_calls = 0usize;
        let mut no_op_strikes = 0usize;
        let mut read_cache: std::collections::HashMap<
            String,
            (usize, u64, std::time::SystemTime),
        > = std::collections::HashMap::new();

        let mut tools_enabled: Vec<String> =
            vec![crate::jockey::worker::TOOL_DIAGNOSTICS.to_string()];
        if self.jj.allow_subagent {
            tools_enabled
                .push(crate::jockey::worker::TOOL_SUBAGENT.to_string());
        }
        if self.jj.allow_web {
            tools_enabled
                .push(crate::jockey::worker::TOOL_WEB_FETCH.to_string());
        }

        let tool_ctx = crate::jockey::tools::ToolCtx {
            sandbox: sandbox.clone(),
            worktree: self.state.worktree.clone(),
            blocklist: self.shell_blocklist.clone(),
            verification_timeout: self.jj.verification_timeout_secs,
            read_lines: self.jj.worker_read_lines,
            allow_web: self.jj.allow_web,
        };

        loop {
            if actions_done >= MAX_ACTIONS_PER_ATTEMPT
                || rejections >= MAX_REJECTIONS_PER_ATTEMPT
            {
                if !records.is_empty() {
                    // Auto-finish: the worker made real changes but stalled;
                    // the governor runs the deterministic gate on its behalf.
                    live_context.push(
                        "GOVERNOR: action cap reached with unverified changes — running verification automatically.".to_string(),
                    );
                    return (
                        self.verify(
                            step_id,
                            Some("governor auto-finish at action cap"),
                        )
                        .await,
                        records,
                    );
                }
                return (
                    AttemptOutcome::NoAction(format!(
                        "attempt aborted: {} actions done, {} rejections (caps {MAX_ACTIONS_PER_ATTEMPT}/{MAX_REJECTIONS_PER_ATTEMPT})",
                        actions_done, rejections
                    )),
                    records,
                );
            }

            let mut prompt_context = live_context.clone();
            if actions_done + 4 >= MAX_ACTIONS_PER_ATTEMPT {
                let no_mutations = records.is_empty();
                let nudge = if no_mutations {
                    "WRAP UP: you are near the action cap for this attempt and have made NO file changes yet. Make the edit now (write_file/edit_file on an allowed path), verify, then call finish_step."
                } else {
                    "WRAP UP: you are near the action cap for this attempt. Finish the current change and call finish_step when the invariants hold."
                };
                prompt_context.push(nudge.to_string());
            }
            if actions_done >= 6 && records.is_empty() {
                prompt_context.push(
                    "REMINDER: reading and searching is not progress. You have not modified any file in this attempt. Implement the step now with edit_file/write_file.".to_string(),
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
                context: prompt_context,
                todos: todos.clone(),
                tools_enabled: tools_enabled.clone(),
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

            const MAX_ACTIONS_PER_TURN: usize = 6;
            let mut effective_this_turn = 0usize;
            for (turn_index, raw_action) in turn.actions.into_iter().enumerate()
            {
                if turn_index >= MAX_ACTIONS_PER_TURN {
                    live_context.push(
                        "NOTE: your reply contained too many tool calls; only the first {MAX_ACTIONS_PER_TURN} were executed. Batch fewer actions.".to_string(),
                    );
                    break;
                }
                let action =
                    relativize_action(raw_action, &self.state.worktree);
                if action.tool == TOOL_FINISH_STEP {
                    let summary = action
                        .args
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("(no summary)")
                        .to_string();
                    return (
                        self.verify(step_id, Some(&summary)).await,
                        records,
                    );
                }

                let brief = crate::jockey::interceptor::action_brief(&action);

                if action.tool == crate::jockey::worker::TOOL_TODO {
                    todo_calls += 1;
                    if todo_calls > 8 {
                        live_context.push(
                            "ACTION (todo): todo budget exhausted — take a real action (edit_file/write_file/run_command) now.".to_string(),
                        );
                        continue;
                    }
                    todos = action
                        .args
                        .get("todos")
                        .and_then(|t| t.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| {
                                    let content = item
                                        .get("content")?
                                        .as_str()?
                                        .to_string();
                                    let status = item
                                        .get("status")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("pending")
                                        .to_string();
                                    Some((content, status))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    live_context.push(format!(
                        "ACTION (todo): list updated ({} items)",
                        todos.len()
                    ));
                    continue;
                }

                if action.tool == crate::jockey::worker::TOOL_SUBAGENT {
                    actions_done += 1;
                    self.emit(JockeyEvent::ActionProposed {
                        step_id: step_id.to_string(),
                        tool: action.tool.clone(),
                        brief: brief.clone(),
                        args: Some(action.args.clone()),
                    });
                    let report = self.run_subagent(&action, &tool_ctx).await;
                    let (ok, text) = report;
                    self.emit(JockeyEvent::ActionExecuted {
                        tool: action.tool.clone(),
                        ok,
                        brief: truncate_str(&text, 200).to_string(),
                    });
                    live_context.push(format!(
                        "ACTION {}: subagent report:\n{}",
                        actions_done,
                        truncate_str(&text, 6000)
                    ));
                    continue;
                }

                self.emit(JockeyEvent::ActionProposed {
                    step_id: step_id.to_string(),
                    tool: action.tool.clone(),
                    brief: brief.clone(),
                    args: Some(action.args.clone()),
                });

                // read dedup: skip re-reading unchanged files
                if action.tool == TOOL_READ_FILE
                    && let Some(path) =
                        action.args.get("path").and_then(|p| p.as_str())
                    && let Ok(resolved) = sandbox.resolve_for_read(path)
                    && let Ok(meta) = std::fs::metadata(&resolved)
                    && let Ok(modified) = meta.modified()
                    && let Some((idx, len, prev_modified)) =
                        read_cache.get(path)
                    && modified == *prev_modified
                {
                    live_context.push(format!(
                        "ACTION (cached): read_file {path} -> (unchanged since ACTION {idx}, \
{len} bytes — already in your context; do something new)"
                    ));
                    continue;
                }

                let failed =
                    self.attempts.get(step_id).cloned().unwrap_or_default();
                let deterministic = self.deterministic_check(&action, sandbox);
                let verdict =
                    interceptor::review(&action, &failed, deterministic);

                match verdict {
                    InterceptVerdict::Approved => {
                        self.emit(JockeyEvent::ActionApproved {
                            tool: action.tool.clone(),
                        });
                        let is_mutating = match action.tool.as_str() {
                            TOOL_WRITE_FILE | TOOL_EDIT_FILE => true,
                            TOOL_RUN_COMMAND => action
                                .args
                                .get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| {
                                    !crate::sandbox::exec::is_readonly_command(
                                        c,
                                    )
                                })
                                .unwrap_or(true),
                            _ => false,
                        };
                        let (ok, result) = self
                            .execute_action(&action, sandbox, &tool_ctx)
                            .await;
                        if action.tool == TOOL_READ_FILE
                            && let Some(path) =
                                action.args.get("path").and_then(|p| p.as_str())
                            && let Ok(resolved) = sandbox.resolve_for_read(path)
                            && let (Ok(meta), Ok(modified)) = (
                                std::fs::metadata(&resolved),
                                std::fs::metadata(&resolved)
                                    .and_then(|m| m.modified()),
                            )
                        {
                            read_cache.insert(
                                path.to_string(),
                                (actions_done + 1, meta.len(), modified),
                            );
                        }
                        self.emit(JockeyEvent::ActionExecuted {
                            tool: action.tool.clone(),
                            ok,
                            brief: truncate_str(&result, 200).to_string(),
                        });
                        actions_done += 1;
                        if is_mutating && ok {
                            records.push(AttemptRecord {
                                fingerprint:
                                    crate::jockey::interceptor::fingerprint(
                                        &action,
                                    ),
                                brief,
                            });
                        }
                        live_context.push(format!(
                            "ACTION {}: {} -> {}",
                            actions_done,
                            crate::jockey::interceptor::action_brief(&action),
                            truncate_str(&result, 4000)
                        ));
                        compact_context(&mut live_context);
                    }
                    InterceptVerdict::Rejected { reason } => {
                        rejections += 1;
                        self.emit(JockeyEvent::ActionRejected {
                            tool: action.tool.clone(),
                            reason: reason.clone(),
                        });
                        let mut guidance = format!(
                            "REJECTED ACTION {}: {} because: {}",
                            rejections,
                            crate::jockey::interceptor::action_brief(&action),
                            reason
                        );
                        if reason.contains("out of scope")
                            || reason.contains("allowed_paths")
                            || reason.contains("outside")
                        {
                            guidance.push_str(&format!(
                                "\nThat work seems to belong to a DIFFERENT step of the plan. Do ONLY this step's invariants: {}\nRe-propose a SMALLER edit focused on this step — do not stop making changes.",
                                invariants.join(" | ")
                            ));
                        }
                        live_context.push(guidance);
                    }
                }
                if matches!(
                    action.tool.as_str(),
                    TOOL_WRITE_FILE
                        | TOOL_EDIT_FILE
                        | TOOL_RUN_COMMAND
                        | TOOL_READ_FILE
                        | crate::jockey::worker::TOOL_GREP
                        | crate::jockey::worker::TOOL_GLOB
                        | crate::jockey::worker::TOOL_LIST
                        | crate::jockey::worker::TOOL_DIAGNOSTICS
                        | crate::jockey::worker::TOOL_SUBAGENT
                        | crate::jockey::worker::TOOL_WEB_FETCH
                ) {
                    effective_this_turn += 1;
                }
            }
            if effective_this_turn == 0 {
                no_op_strikes += 1;
                live_context.push(
                    "WARNING: that reply made no effective progress (only repeats/todo). Propose an edit_file, write_file, run_command, or a NEW search now.".to_string(),
                );
                if no_op_strikes >= 3 {
                    return (
                        AttemptOutcome::NoAction(
                            "worker repeated already-done actions and todo updates without making progress".to_string(),
                        ),
                        records,
                    );
                }
            } else {
                no_op_strikes = 0;
            }
        }
    }

    /// Read-only explorer subagent: bounded loop, read/search tools only,
    /// terminal `report`. Returns its findings for the main context.
    async fn run_subagent(
        &mut self,
        action: &WorkerAction,
        tool_ctx: &crate::jockey::tools::ToolCtx,
    ) -> (bool, String) {
        let Some(task) = action.args.get("task").and_then(|t| t.as_str())
        else {
            return (false, "subagent: missing task".to_string());
        };
        let system = "You are a READ-ONLY explorer subagent inside a coding workspace. \
You have read_file, grep, glob, list, and report tools — nothing else; you cannot modify anything. \
Gather precisely what the task asks using grep/glob first (cheap), reading only what you must. \
Then call report ONCE with a dense, factual answer (max 400 words, include file paths and signatures).";
        let defs = Worker::subagent_definitions();
        let tools: Vec<String> =
            defs.iter().map(|d| d.function.name.clone()).collect();
        let mut history: Vec<String> = Vec::new();
        for _round in 0..8 {
            let full_task = if history.is_empty() {
                format!(
                    "TASK: {task}\n\nCall read/grep/glob tools now, or report when you have the answer."
                )
            } else {
                format!(
                    "TASK: {task}\n\nYour actions so far:\n{}\nContinue with tools or call report now.",
                    history.join("\n")
                )
            };
            let turn = match self
                .worker
                .propose_subagent(system, &full_task, &tools, defs.clone())
                .await
            {
                Ok(t) => t,
                Err(e) => return (false, format!("subagent failed: {e}")),
            };
            self.state.ledger.add_worker(turn.usage.as_ref());
            for act in turn.actions {
                if act.tool == crate::jockey::worker::TOOL_REPORT {
                    let summary = act
                        .args
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("(empty report)")
                        .to_string();
                    return (true, summary);
                }
                let (ok, result) = tool_ctx.execute(&act).await;
                history.push(format!(
                    "{} {} -> {}",
                    act.tool,
                    crate::jockey::interceptor::action_brief(&act),
                    truncate_str(&result, 1500)
                ));
                let _ = ok;
            }
        }
        (
            false,
            "subagent exhausted its 8 action rounds without reporting"
                .to_string(),
        )
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
        _sandbox: &PathSandbox,
        tool_ctx: &crate::jockey::tools::ToolCtx,
    ) -> (bool, String) {
        tool_ctx.execute(action).await
    }

    /// Step completion gate: the deterministic verification command (if
    /// planned), then AiWrangler's `wrangler_check` battery over the dirty
    /// worktree (scope discipline, claims vs diff, injection defense,
    /// test quality). PASS checkpoints; FIX REQUIRED feeds cited findings
    /// back as the next attempt's error; HUMAN REVIEW parks the step for
    /// the owner; DEGRADED/unreachable fail closed unless allow_degraded.
    async fn verify(
        &mut self,
        step_id: &str,
        finish_summary: Option<&str>,
    ) -> AttemptOutcome {
        let step = self.state.dag.step(step_id);
        let (command, description, invariants) = match step {
            Some(s) => (
                s.verification_command.clone(),
                s.description.clone(),
                s.invariants.clone(),
            ),
            None => (None, String::new(), vec![]),
        };

        let mut tool_output = String::new();
        if let Some(command) = &command {
            self.emit(JockeyEvent::VerificationStarted {
                step_id: step_id.to_string(),
                command: command.clone(),
            });
            let start = std::time::Instant::now();
            let outcome = run_command(
                &self.state.worktree,
                command,
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

            if !outcome.success {
                let output = outcome.combined();
                self.emit(JockeyEvent::VerificationFailed {
                    step_id: step_id.to_string(),
                    output: truncate_str(&output, 2000).to_string(),
                });
                return AttemptOutcome::VerificationFailed(output);
            }
            tool_output = truncate_str(&outcome.combined(), 4000).to_string();
            self.emit(JockeyEvent::VerificationPassed {
                step_id: step_id.to_string(),
                commit: String::new(),
                duration_secs: start.elapsed().as_secs_f64(),
            });
        }

        // AiWrangler check gate over the dirty worktree.
        let task = format!(
            "step {step_id}: {description}\ninvariants: {}",
            invariants.join(" | ")
        );
        let verdict = self
            .wrangler
            .check(
                Some(&task),
                finish_summary,
                (!tool_output.is_empty()).then_some(tool_output.as_str()),
            )
            .await;
        self.state.ledger.add_wrangler(verdict.usage);
        self.emit(JockeyEvent::Usage {
            ledger: self.state.ledger.clone(),
        });
        self.emit(JockeyEvent::WranglerGate {
            gate: "check".to_string(),
            action: format!("{:?}", verdict.action),
            brief: truncate_str(verdict.raw.lines().next().unwrap_or(""), 200)
                .to_string(),
        });

        let findings = excerpt_findings(&verdict.raw);
        match verdict.action {
            CheckAction::Pass => AttemptOutcome::Verified,
            CheckAction::FixRequired => AttemptOutcome::VerificationFailed(
                format_wrangler_failure("fix required", &verdict, &findings),
            ),
            CheckAction::HumanReview => {
                AttemptOutcome::NeedsOwner(format_wrangler_failure(
                    "human review required",
                    &verdict,
                    &findings,
                ))
            }
            CheckAction::Degraded => {
                if findings.is_empty() {
                    if self.jj.allow_degraded {
                        AttemptOutcome::Verified
                    } else {
                        AttemptOutcome::VerificationFailed(
                            format_wrangler_failure(
                                "semantic gate degraded",
                                &verdict,
                                "",
                            ),
                        )
                    }
                } else {
                    // Deterministic findings still block even degraded.
                    AttemptOutcome::VerificationFailed(format_wrangler_failure(
                        "fix required (degraded)",
                        &verdict,
                        &findings,
                    ))
                }
            }
            CheckAction::Inactive | CheckAction::Unreachable => {
                if self.jj.allow_degraded {
                    AttemptOutcome::Verified
                } else {
                    AttemptOutcome::VerificationFailed(format!(
                        "AiWrangler check gate unavailable — failing closed. \
Set --allow-degraded to accept deterministic-only verification.\n{}",
                        truncate_str(&verdict.raw, 1000)
                    ))
                }
            }
        }
    }

    /// Triage of a failed attempt. The old Jev choice call is gone;
    /// classification is deterministic: an error that points at a file
    /// the worker hasn't seen → inject it (read_context); AiWrangler's
    /// open circuit breaker (same findings cycling) or a hopeless repeat
    /// → deadlock → frontier escalation; anything else retries with the
    /// failure output inline (syntax_fix).
    async fn triage(
        &mut self,
        step_id: &str,
        _description: &str,
        _invariants: &[String],
        error: &Option<String>,
    ) -> TriageVerdict {
        let error_text = error.as_deref().unwrap_or("");
        let verdict = if error_text.contains("CIRCUIT BREAKER OPEN") {
            TriageVerdict::Deadlock
        } else if self.read_context_for_error(error).is_some() {
            TriageVerdict::ReadContext
        } else {
            TriageVerdict::SyntaxFix
        };
        self.emit(JockeyEvent::Triage {
            step_id: step_id.to_string(),
            verdict: format!("{verdict:?}").to_lowercase(),
            confidence: 1.0,
        });
        verdict
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
model is stuck on ONE step. You get the step, its invariants, the relevant files, the worker's \
recent actions, and the exact failure. Reply with EITHER:\n\
(1) a unified diff in a ```diff fence (paths relative to repo root) that fixes the step, OR\n\
(2) a paragraph starting with GUIDANCE: giving the local model concrete, actionable instructions.\n\
Never restate the problem; fix or direct. Keep it minimal.\n";
        let action_log = self
            .attempts
            .get(step_id)
            .map(|records| {
                records
                    .iter()
                    .rev()
                    .take(10)
                    .rev()
                    .map(|r| format!("- {}", r.brief))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "(no recorded mutating actions)".to_string());
        let user = format!(
            "STEP [{step_id}]: {description}\nINVARIANTS:\n{}\n\nFILES:\n{files_block}\n\nWORKER MUTATING ACTIONS SO FAR:\n{action_log}\n\nFAILURE:\n{}",
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
                                self.verify(
                                    step_id,
                                    Some("frontier escalation patch"),
                                )
                                .await,
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
    /// AiWrangler returned HUMAN REVIEW: the step parks until the owner
    /// runs `seekr ack <run-id>`; no further local attempts.
    NeedsOwner(String),
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

/// Extracts the actionable findings sections from a wrangler check
/// report (deterministic + semantic), skipping verdict/usage noise.
fn excerpt_findings(report: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_findings = false;
    for line in report.lines() {
        let t = line.trim();
        if t.starts_with("deterministic findings:")
            || t.starts_with("semantic findings:")
        {
            in_findings = true;
            continue;
        }
        if in_findings {
            if t.is_empty() {
                // A blank line ends the block unless another findings
                // section follows; handled by the next non-empty check.
                out.push("");
                continue;
            }
            if t.ends_with(':') && !t.starts_with("  ") && !t.starts_with("- ")
            {
                // Next section header (e.g. "routing reasons:", "context
                // pack:") ends the excerpt.
                break;
            }
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Formats a wrangler check failure for the worker's next attempt.
fn format_wrangler_failure(
    label: &str,
    verdict: &crate::wrangler::CheckVerdict,
    findings: &str,
) -> String {
    let mut out = format!(
        "AIWRANGLER CHECK: {label}\n{}",
        truncate_str(&verdict.raw, 1500)
    );
    if !findings.is_empty() {
        out.push_str(&format!(
            "\n\nCITED FINDINGS (fix every one, then finish_step again):\n{}",
            truncate_str(findings, 3000)
        ));
    }
    out
}

/// Rewrites absolute worktree-rooted paths in action arguments to
/// workspace-relative form, keeping fingerprints canonical.
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

/// Keeps the newest 3 action results verbatim; older ones collapse to a
/// one-line header past the budget so the worker context stays bounded.
fn compact_context(context: &mut [String]) {
    const BUDGET: usize = 24_000;
    let total: usize = context.iter().map(|c| c.len()).sum();
    if total <= BUDGET {
        return;
    }
    let action_idx: Vec<usize> = context
        .iter()
        .enumerate()
        .filter(|(_, c)| c.starts_with("ACTION "))
        .map(|(i, _)| i)
        .collect();
    let keep_full: std::collections::HashSet<usize> =
        action_idx.iter().rev().take(3).copied().collect();
    for i in action_idx {
        if !keep_full.contains(&i) && context[i].len() > 300 {
            let first_line =
                context[i].lines().next().unwrap_or("").to_string();
            context[i] = format!("{first_line} …(compacted)");
        }
    }
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
