use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use seekr::config::AppConfig;
use seekr::jockey::cli;
use seekr::jockey::driver::{RunOutcome, RunState};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "seekr — autonomous, cost-guarded coding agent; gates by AiWrangler",
    long_about = None
)]
struct Cli {
    /// Repository to work in (default: current directory)
    path: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start an autonomous run (TUI by default; headless when piped or --headless)
    Run {
        /// Repository to work in (default: current directory)
        path: Option<std::path::PathBuf>,
        /// Goal for the frontier planner ('-' reads stdin; omit with --plan)
        #[arg(short, long)]
        goal: Option<String>,
        /// Load a TaskDag JSON file instead of frontier planning
        #[arg(short, long)]
        plan: Option<std::path::PathBuf>,
        /// Force headless mode
        #[arg(long)]
        headless: bool,
        /// Pre-supplied clarification answer (`id=text`, repeatable)
        #[arg(long = "answer")]
        answers: Vec<String>,
        /// Proceed without interactive clarification
        #[arg(long)]
        auto: bool,
        /// Permit autonomous runs without the AiWrangler semantic layer (fail-closed default)
        #[arg(long)]
        allow_degraded: bool,
        /// Emit raw JSONL events instead of human output (headless only)
        #[arg(long)]
        json: bool,
    },
    /// List previous runs
    Runs,
    /// Resume an interrupted run
    Resume { run_id: String },
    /// Show a run's event log (most recent last)
    Logs {
        run_id: String,
        #[arg(short, long, default_value = "40")]
        lines: usize,
    },
    /// Merge a finished run's branch back and clean up its worktree
    Merge { run_id: String },
    /// Remove a run's worktree, keeping its branch
    Clean { run_id: String },
    /// Delete a run's state (and worktree); the branch is kept
    Rm { run_id: String },
    /// The return-after-hours view: attention-first summary of a run
    Summary {
        run_id: String,
        /// Append the raw event tail
        #[arg(long)]
        verbose: bool,
    },
    /// Review / acknowledge AiWrangler human-review findings for a run
    Ack {
        run_id: String,
        /// Rule IDs to acknowledge, e.g. RULE-10,RULE-11 (omit to list)
        #[arg(long, value_delimiter = ',')]
        rules: Vec<String>,
        /// What the owner approved and why (audited)
        #[arg(long)]
        note: Option<String>,
    },
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
    /// Generate shell completions
    Completions { shell: clap_complete::Shell },
}

fn resolve_repo(
    path: Option<&std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    match path {
        Some(p) => Ok(p.canonicalize().unwrap_or_else(|_| p.clone())),
        None => Ok(std::env::current_dir()?),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load().unwrap_or_default();

    match cli.command {
        None => {
            let repo = resolve_repo(cli.path.as_ref())?;
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
            {
                seekr::ui::jockey::run_tui_new(config, repo).await
            } else {
                eprintln!(
                    "seekr: no terminal available.\n\
                     Try:  seekr .                      (in a terminal)\n\
                           seekr run . --goal \"...\" --headless\n\
                           seekr --help"
                );
                Ok(())
            }
        }
        Some(Commands::Run {
            path,
            goal,
            plan,
            headless,
            answers,
            auto,
            allow_degraded,
            json,
        }) => {
            let repo = resolve_repo(path.as_ref())?;
            run_command(
                goal,
                plan,
                repo,
                headless,
                answers,
                auto,
                allow_degraded,
                json,
                config,
            )
            .await
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
        Some(Commands::Logs { run_id, lines }) => {
            let dir = RunState::state_dir(&run_id).ok_or_else(|| {
                anyhow::anyhow!("cannot determine state directory")
            })?;
            let raw = std::fs::read_to_string(dir.join("events.jsonl"))?;
            let all: Vec<&str> = raw.lines().collect();
            for line in all.iter().rev().take(lines).rev() {
                match serde_json::from_str::<serde_json::Value>(line) {
                    Ok(v) => println!("{v}"),
                    Err(_) => println!("{line}"),
                }
            }
            Ok(())
        }
        Some(Commands::Merge { run_id }) => cli::merge_run(&run_id).await,
        Some(Commands::Clean { run_id }) => cli::clean_run(&run_id).await,
        Some(Commands::Rm { run_id }) => cli::rm_run(&run_id).await,
        Some(Commands::Summary { run_id, verbose }) => {
            cli::summary_run(&run_id, verbose)
        }
        Some(Commands::Ack {
            run_id,
            rules,
            note,
        }) => cli::ack_run(&run_id, &rules, note.as_deref()).await,
        Some(Commands::Doctor) => cli::doctor().await,
        Some(Commands::Plan { goal, repo, auto }) => {
            let repo = resolve_repo(repo.as_ref())?;
            let roles = cli::resolve_roles(&config)?;
            let dag = cli::plan_interactive(&roles, &goal, &repo, &[], auto, 2)
                .await?;
            println!("{}", serde_json::to_string_pretty(&dag)?);
            Ok(())
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            clap_complete::generate(
                shell,
                &mut cmd,
                "seekr",
                &mut std::io::stdout(),
            );
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    goal: Option<String>,
    plan: Option<std::path::PathBuf>,
    repo: std::path::PathBuf,
    headless: bool,
    answers: Vec<String>,
    auto: bool,
    allow_degraded: bool,
    json: bool,
    mut config: AppConfig,
) -> Result<()> {
    if allow_degraded {
        config.jockey.allow_degraded = true;
    }
    let roles = cli::resolve_roles(&config)?;

    let goal = match goal {
        Some(g) if g == "-" => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Some(buf.trim().to_string())
        }
        other => other,
    };

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
        (None, None) => {
            anyhow::bail!("provide --goal (or '-' for stdin) or --plan")
        }
    };

    let interactive = !headless
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    if interactive {
        seekr::ui::jockey::run_tui_with_dag(config, roles, goal, dag, repo)
            .await
    } else {
        let run_id = cli::new_run_id();
        if json {
            cli::run_headless_json(&config, &roles, &goal, dag, &repo, run_id)
                .await?;
        } else {
            cli::run_headless(&config, &roles, &goal, dag, &repo, run_id)
                .await?;
        }
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
        cli::worker_reasoning(&config),
    );
    let mut governor = seekr::jockey::driver::Governor::attach(
        &config,
        worker,
        roles.frontier.clone(),
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
