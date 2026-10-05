use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use crate::api::client::ApiClient;
use crate::config::AppConfig;
use crate::jev::client::JevClient;
use crate::jockey::dag::TaskDag;
use crate::jockey::driver::{Governor, RunOutcome, RunState};
use crate::jockey::planner::{FrontierPlanner, PlanOutcome, PlanResponse};
use crate::jockey::worker::Worker;
use crate::jockey::JockeyEvent;
use crate::sandbox::git::GitSandbox;

#[derive(Clone)]
pub struct Roles {
    pub worker: (ApiClient, String),
    pub frontier: Option<(ApiClient, String)>,
    pub jev: JevClient,
}

/// Resolves worker/frontier provider roles and the Jev client from config
/// + environment. Fails fast with actionable errors.
pub fn resolve_roles(config: &AppConfig) -> Result<Roles> {
    let worker_cfg = match &config.jockey.worker_provider {
        Some(name) => config.provider_by_name(name).ok_or_else(|| {
            anyhow::anyhow!(
                "[jj] worker_provider = '{name}' not found in [[providers]]; add it to config.toml"
            )
        })?,
        None => config.current_provider(),
    };
    let worker = (
        ApiClient::new_for_provider(config, worker_cfg),
        worker_cfg.model.clone(),
    );

    let frontier = match &config.jockey.frontier_provider {
        Some(name) => match config.provider_by_name(name) {
            Some(cfg) => Some((ApiClient::new_for_provider(config, cfg), cfg.model.clone())),
            None => {
                eprintln!(
                    "warn: [jj] frontier_provider = '{name}' not found; escalation disabled"
                );
                None
            }
        },
        None => None,
    };

    let jev = JevClient::from_env();
    if let Some(reason) = jev.unavailable() {
        eprintln!(
            "warn: jev semantic gate unavailable ({reason:?}); autonomous writes will be \
blocked unless allow_degraded is set"
        );
    }

    Ok(Roles {
        worker,
        frontier,
        jev,
    })
}

pub fn new_run_id() -> String {
    format!(
        "jj-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
    ) + &uuid::Uuid::new_v4().simple().to_string()[..6]
}

/// Loads a DAG from a plan file, validating it.
pub fn load_plan(path: &Path) -> Result<TaskDag> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading plan {}", path.display()))?;
    let dag: TaskDag =
        serde_json::from_str(&raw).context("plan file is not valid TaskDag JSON")?;
    dag.validate().map_err(|e| anyhow::anyhow!("invalid plan: {e}"))?;
    Ok(dag)
}

/// Phase 0: plan via frontier, running clarification rounds over stdin.
/// `answers` carries pre-supplied answers ("id=text"); `auto` proceeds
/// without human input when clarification is requested.
pub async fn plan_interactive(
    roles: &Roles,
    goal: &str,
    repo_root: &Path,
    pre_answers: &[(String, String)],
    auto: bool,
    max_rounds: u32,
) -> Result<TaskDag> {
    let (client, model) = roles
        .frontier
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no frontier provider configured; use --plan instead"))?;
    let planner = FrontierPlanner::new(client, model, 0.2, 8192);
    let tree = crate::jockey::planner::collect_repo_tree(repo_root, 400);

    let mut answers: Vec<(String, String)> = pre_answers.to_vec();
    for _round in 0..max_rounds {
        let response: PlanResponse = planner.plan(goal, &answers, &tree).await?;
        match response.outcome {
            PlanOutcome::Ready(dag) => return Ok(*dag),
            PlanOutcome::NeedsClarification(questions) => {
                if auto {
                    eprintln!("--auto: proceeding without answering {} clarification(s)", questions.len());
                    break;
                }
                for q in &questions {
                    if answers.iter().any(|(id, _)| id == &q.id) {
                        continue;
                    }
                    if !q.why.is_empty() {
                        println!("  ({})", q.why);
                    }
                    println!("{} ", q.question);
                    let mut input = String::new();
                    std::io::stdin()
                        .read_line(&mut input)
                        .context("reading clarification answer")?;
                    answers.push((q.id.clone(), input.trim().to_string()));
                }
            }
        }
    }
    // final round: force a DAG
    let response = planner.plan(goal, &answers, &tree).await?;
    match response.outcome {
        PlanOutcome::Ready(dag) => Ok(*dag),
        PlanOutcome::NeedsClarification(_) => {
            Err(anyhow::anyhow!("planner still needs clarification after {max_rounds} rounds"))
        }
    }
}

/// Headless autonomous run. Prints events as they arrive.
pub async fn run_headless(
    config: &AppConfig,
    roles: &Roles,
    goal: &str,
    dag: TaskDag,
    repo_root: &Path,
    run_id: String,
) -> Result<RunOutcome> {
    let sandbox = GitSandbox::create(repo_root, &run_id).await?;
    println!("run {} on branch {} in {}", run_id, sandbox.branch(), sandbox.worktree().display());

    let worker = Worker::new(
        roles.worker.0.clone(),
        roles.worker.1.clone(),
        config.jockey.worker_temperature,
        config.jockey.worker_max_tokens,
        std::env::var("SEEKR_WORKER_REASONING").ok(),
    );
    let mut governor = Governor::new(
        config,
        worker,
        roles.frontier.clone(),
        roles.jev.clone(),
        sandbox,
        goal.to_string(),
        dag,
        run_id.clone(),
    )?;
    let mut rx = governor.take_event_rx();

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            print_event(&event);
        }
    });

    let outcome = governor.run_to_completion().await?;
    printer.abort();
    print_summary(&governor.state);
    Ok(outcome)
}

pub fn print_event(event: &JockeyEvent) {
    use crate::jockey::ledger::JockeyEvent::*;
    match event {
        RunStarted { run_id, goal } => println!("▶ run {run_id}: {goal}"),
        PlanReady { .. } => {}
        StepStarted { step_id, attempt } => println!("▶ step {step_id} (attempt {attempt})"),
        ActionProposed { tool, brief, .. } => println!("  … {tool}: {brief}"),
        ActionApproved { tool, scope_p, novelty } => {
            let extra = match (scope_p, novelty) {
                (Some(s), Some(n)) => format!(" [scope {s:.2} novelty {n:.1}]"),
                (Some(s), None) => format!(" [scope {s:.2}]"),
                _ => String::new(),
            };
            println!("  ✓ {tool} approved{extra}");
        }
        ActionRejected { tool, reason } => println!("  ✗ {tool} rejected: {reason}"),
        ActionExecuted { tool, ok, brief } => {
            let mark = if *ok { "✔" } else { "✘" };
            println!("  {mark} {tool} executed");
            if !*ok {
                println!("    {}", first_lines(brief, 3));
            }
        }
        VerificationStarted { step_id, command } => println!("  ⏳ verify [{step_id}]: {command}"),
        VerificationPassed { step_id, duration_secs, .. } => {
            println!("  ✓ [{step_id}] verified in {duration_secs:.1}s")
        }
        VerificationFailed { step_id, output } => {
            println!("  ✗ [{step_id}] verification failed:");
            println!("    {}", first_lines(output, 6));
        }
        RolledBack { step_id, to_commit } => println!("  ↺ [{step_id}] rolled back to {to_commit}"),
        Triage { step_id, verdict, confidence } => {
            println!("  ⚕ [{step_id}] triage: {verdict} ({confidence:.2})")
        }
        Escalated { step_id, reason } => println!("  ↑ [{step_id}] escalated: {reason}"),
        FrontierGuidance { step_id, brief } => println!("  ↑ [{step_id}] frontier: {brief}"),
        StepCompleted { step_id, commit } => println!("✓ step {step_id} checkpointed at {commit}"),
        StepFailed { step_id, reason } => println!("✗ step {step_id} failed: {reason}"),
        Usage { .. } => {}
        RunCompleted { status, branch, worktree, .. } => println!(
            "✔ run {status} — branch {branch}, worktree {}",
            worktree.display()
        ),
        RunFailed { reason, .. } => println!("✗ run failed: {reason}"),
    }
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n    ")
}

pub fn print_summary(state: &RunState) {
    println!("\n── cost ledger ─────────────────────────────");
    println!(
        "worker : {} calls, {} prompt + {} completion tokens",
        state.ledger.worker_calls, state.ledger.worker_prompt_tokens, state.ledger.worker_completion_tokens
    );
    println!(
        "jev    : {} calls (+{} cached), {} in + {} out tokens",
        state.ledger.jev_calls, state.ledger.jev_cache_hits, state.ledger.jev_input_tokens, state.ledger.jev_output_tokens
    );
    println!(
        "frontier: {} calls, {} prompt + {} completion tokens",
        state.ledger.frontier_calls, state.ledger.frontier_prompt_tokens, state.ledger.frontier_completion_tokens
    );
    println!("steps  :");
    for step in &state.dag.steps {
        println!("  {:?} {}", step.status, step.id);
    }
    println!("\nreview : cd {} && git log / git diff main..{}", state.worktree.display(), state.branch);
    println!("merge  : seekr merge {}", state.run_id);
}

/// `seekr merge <run_id>`: merges the run branch back into the repo's
/// current branch and disposes the worktree (branch is preserved).
pub async fn merge_run(run_id: &str) -> Result<()> {
    let state = RunState::load(run_id).context("run not found")?;
    let repo = &state.repo_root;
    let sandbox = tokio::process::Command::new("git")
        .args(["-C", &repo.display().to_string(), "merge", "--no-ff", &state.branch, "-m", &format!("merge {run_id}")])
        .output()
        .await
        .context("git merge")?;
    if !sandbox.status.success() {
        anyhow::bail!(
            "merge failed: {}",
            String::from_utf8_lossy(&sandbox.stderr)
        );
    }
    cleanup_worktree(&state).await;
    println!("merged {run_id} into {}", repo.display());
    Ok(())
}

pub async fn cleanup_worktree(state: &RunState) {
    let _ = tokio::process::Command::new("git")
        .args([
            "-C",
            &state.repo_root.display().to_string(),
            "worktree",
            "remove",
            "--force",
            &state.worktree.display().to_string(),
        ])
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .args([
            "-C",
            &state.repo_root.display().to_string(),
            "worktree",
            "prune",
        ])
        .output()
        .await;
}

/// `seekr clean <run_id>`: dispose the worktree, keep the branch.
pub async fn clean_run(run_id: &str) -> Result<()> {
    let state = RunState::load(run_id).context("run not found")?;
    cleanup_worktree(&state).await;
    println!(
        "worktree removed; branch {} kept for later merge",
        state.branch
    );
    Ok(())
}

/// Sanity checks with actionable output (replaces the old doctor).
pub async fn doctor() -> Result<()> {
    println!("seekr {}\n", env!("CARGO_PKG_VERSION"));

    let config = AppConfig::load().context("config load failed")?;
    println!(
        "config      : {}",
        AppConfig::config_path()?.display()
    );
    println!("providers   : {} configured", config.providers.len());
    for p in &config.providers {
        let key_state = if p.key.is_empty() { "NO KEY" } else { "key ok" };
        println!("  - {:<14} {:<40} {key_state}", p.name, p.model);
    }
    match (&config.jockey.worker_provider, &config.jockey.frontier_provider) {
        (Some(w), Some(f)) => println!("roles       : worker={w} frontier={f}"),
        (Some(w), None) => println!("roles       : worker={w} frontier=<none>"),
        _ => println!("roles       : worker=<active provider> (set [jj] worker_provider)"),
    }

    let jev = JevClient::from_env();
    match jev.unavailable() {
        None => {
            let set = crate::jev::QuestionSet::new().add(
                "health",
                crate::jev::noul(crate::jev::instructions("Is 1+1=2?", "arithmetic"), None, None),
            );
            match jev.ask(&json!({"probe": "doctor"}), &set).await {
                Ok(r) => println!("jev         : ok (model {})", r.model),
                Err(e) => println!("jev         : KEY SET BUT CALL FAILED: {e}"),
            }
        }
        Some(reason) => println!("jev         : unavailable ({reason:?}) — writes will be blocked"),
    }

    let git = tokio::process::Command::new("git")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!("git         : {}", if git { "ok" } else { "MISSING" });
    Ok(())
}
