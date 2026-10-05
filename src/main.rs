use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use seekr::config::AppConfig;
use seekr::jockey::cli;
use seekr::jockey::driver::{RunOutcome, RunState};

#[derive(Parser, Debug)]
#[command(version, about = "Jev Jockey — autonomous, cost-guarded coding agent", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start an autonomous run (TUI by default; headless when piped or --headless)
    Run {
        /// Goal for the frontier planner (omit with --plan)
        #[arg(short, long)]
        goal: Option<String>,
        /// Load a TaskDag JSON file instead of frontier planning
        #[arg(short, long)]
        plan: Option<std::path::PathBuf>,
        /// Repository to work in (default: cwd)
        #[arg(short, long)]
        repo: Option<std::path::PathBuf>,
        /// Force headless mode
        #[arg(long)]
        headless: bool,
        /// Pre-supplied clarification answer (`id=text`, repeatable)
        #[arg(long = "answer")]
        answers: Vec<String>,
        /// Proceed without interactive clarification
        #[arg(long)]
        auto: bool,
        /// Permit autonomous writes without the Jev gate (fail-closed default)
        #[arg(long)]
        allow_degraded: bool,
    },
    /// List previous runs
    Runs,
    /// Resume an interrupted run
    Resume { run_id: String },
    /// Merge a finished run's branch back and clean up its worktree
    Merge { run_id: String },
    /// Remove a run's worktree, keeping its branch
    Clean { run_id: String },
    /// Environment and configuration diagnostics
    Doctor,
    /// Print a TaskDag JSON for a goal without running it
    Plan {
        #[arg(short, long)]
        goal: String,
        #[arg(short, long)]
        repo: Option<std::path::PathBuf>,
        #[arg(long)]
        auto: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load().unwrap_or_default();

    match cli.command {
        None | Some(Commands::Run { .. }) => {
            if let Some(Commands::Run {
                goal,
                plan,
                repo,
                headless,
                answers,
                auto,
                allow_degraded,
            }) = cli.command
            {
                run_command(
                    goal,
                    plan,
                    repo,
                    headless,
                    answers,
                    auto,
                    allow_degraded,
                    config,
                )
                .await
            } else {
                seekr::ui::jockey::run_tui_new(config).await
            }
        }
        Some(Commands::Runs) => {
            for (id, status) in RunState::list_runs() {
                println!("{id:<28} {status}");
            }
            Ok(())
        }
        Some(Commands::Resume { run_id }) => {
            let state = RunState::load(&run_id)?;
            println!(
                "resuming {run_id} ({}) on branch {}",
                state.status, state.branch
            );
            resume_headless(state, config).await
        }
        Some(Commands::Merge { run_id }) => cli::merge_run(&run_id).await,
        Some(Commands::Clean { run_id }) => cli::clean_run(&run_id).await,
        Some(Commands::Doctor) => cli::doctor().await,
        Some(Commands::Plan { goal, repo, auto }) => {
            let repo = match repo {
                Some(p) => p.canonicalize().unwrap_or(p),
                None => std::env::current_dir()?,
            };
            let roles = cli::resolve_roles(&config)?;
            let dag = cli::plan_interactive(&roles, &goal, &repo, &[], auto, 2)
                .await?;
            println!("{}", serde_json::to_string_pretty(&dag)?);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    goal: Option<String>,
    plan: Option<std::path::PathBuf>,
    repo: Option<std::path::PathBuf>,
    headless: bool,
    answers: Vec<String>,
    auto: bool,
    allow_degraded: bool,
    mut config: AppConfig,
) -> Result<()> {
    if allow_degraded {
        config.jockey.allow_degraded = true;
    }
    let repo = match repo {
        Some(p) => p.canonicalize().unwrap_or(p),
        None => std::env::current_dir()?,
    };
    let roles = cli::resolve_roles(&config)?;

    let (goal, dag) = match (goal, plan) {
        (_, Some(plan_path)) => {
            let dag = cli::load_plan(&plan_path)?;
            let goal = dag.goal.clone();
            (goal, dag)
        }
        (Some(goal), None) => {
            let parsed: Vec<(String, String)> = answers
                .iter()
                .filter_map(|a| {
                    a.split_once('=').map(|(k, v)| {
                        (k.trim().to_string(), v.trim().to_string())
                    })
                })
                .collect();
            let dag =
                cli::plan_interactive(&roles, &goal, &repo, &parsed, auto, 2)
                    .await?;
            (goal, dag)
        }
        (None, None) => anyhow::bail!("provide --goal or --plan"),
    };

    let interactive = !headless
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    if interactive {
        seekr::ui::jockey::run_tui_with_dag(config, roles, goal, dag, repo)
            .await
    } else {
        let run_id = cli::new_run_id();
        cli::run_headless(&config, &roles, &goal, dag, &repo, run_id).await?;
        Ok(())
    }
}

async fn resume_headless(state: RunState, config: AppConfig) -> Result<()> {
    let roles = cli::resolve_roles(&config)?;
    let sandbox = seekr::sandbox::git::GitSandbox::attach(
        &state.repo_root,
        &state.run_id,
    )?;
    let worker = seekr::jockey::worker::Worker::new(
        roles.worker.0.clone(),
        roles.worker.1.clone(),
        config.jockey.worker_temperature,
        config.jockey.worker_max_tokens,
        std::env::var("SEEKR_WORKER_REASONING").ok(),
    );
    let mut governor = seekr::jockey::driver::Governor::attach(
        &config,
        worker,
        roles.frontier.clone(),
        roles.jev.clone(),
        sandbox,
        state,
    )?;
    let mut rx = governor.take_event_rx();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            cli::print_event(&event);
        }
    });
    let outcome = governor.run_to_completion().await?;
    printer.abort();
    cli::print_summary(&governor.state);
    match outcome {
        RunOutcome::Success => Ok(()),
        RunOutcome::Failed(reason) => Err(anyhow::anyhow!("{reason}")),
    }
}
