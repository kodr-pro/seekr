use anyhow::Result;
use clap::{Parser, Subcommand};
use seekr::daemon::server::start_server;
use seekr::{app, config};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the background Seekr daemon
    Daemon {
        #[command(subcommand)]
        subcommand: Option<DaemonCommands>,
    },
    /// Run diagnostics
    Doctor,
    /// Start TUI from a previous session
    Resume { session_id: String },
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Check if the daemon is running
    Status,
    /// Stop a running daemon
    Stop,
    /// Show recent daemon logs
    Logs {
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    setup_panic_hook();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon { subcommand }) => match subcommand {
            None => {
                println!("Starting Seekr daemon...");
                return start_server().await;
            }
            Some(DaemonCommands::Status) => {
                return daemon_status().await;
            }
            Some(DaemonCommands::Stop) => {
                return daemon_stop().await;
            }
            Some(DaemonCommands::Logs { lines }) => {
                return daemon_logs(lines);
            }
        },
        Some(Commands::Doctor) => {
            return seekr::doctor::run_diagnostics().await;
        }
        _ => {}
    }

    let resume_id = match cli.command {
        Some(Commands::Resume { ref session_id }) => Some(session_id.clone()),
        _ => None,
    };

    if matches!(cli.command, None | Some(Commands::Resume { .. })) {
        let client = seekr::daemon::client::DaemonClient::new();
        if !client.check_health().await {
            println!("Daemon not reachable. Starting 'seekr daemon' in background...");
            if let Ok(exe) = std::env::current_exe() {
                let _ = tokio::process::Command::new(exe).arg("daemon").spawn();

                let mut retries = 0;
                while !client.check_health().await && retries < 30 {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    retries += 1;
                }

                if retries >= 30 {
                    eprintln!("Warning: Daemon failed to start within 3 seconds.");
                }
            }
        }
    }

    let mut app = if config::AppConfig::exists() {
        match config::AppConfig::load() {
            Ok(cfg) => app::App::new_main(cfg),
            Err(e) => {
                eprintln!("Failed to load config: {}. Starting setup wizard.", e);
                app::App::new_setup()
            }
        }
    } else {
        app::App::new_setup()
    };

    if let Some(sid) = resume_id
        && app.mode == app::AppMode::Main
    {
        app.resume_session(sid);
    }

    app::run_app(app).await
} // main

async fn daemon_status() -> Result<()> {
    let client = seekr::daemon::client::DaemonClient::new();
    if client.check_health().await {
        println!("Seekr daemon is running");
        if let Some(pid) = seekr::daemon::server::read_pid_file() {
            println!("  PID: {}", pid);
        }
        if let Some(path) = seekr::daemon::server::pid_file_path() {
            println!("  PID file: {}", path.display());
        }
    } else {
        println!("Seekr daemon is NOT running");
        if seekr::daemon::server::read_pid_file().is_some() {
            println!("  (stale PID file found - removing)");
            seekr::daemon::server::remove_pid_file();
        }
    }
    Ok(())
}

async fn daemon_stop() -> Result<()> {
    let client = seekr::daemon::client::DaemonClient::new();
    if !client.check_health().await {
        println!("Daemon is not running.");
        seekr::daemon::server::remove_pid_file();
        return Ok(());
    }

    client.send_shutdown().await?;
    println!("Shutdown signal sent to daemon.");

    let mut retries = 0;
    while client.check_health().await && retries < 30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        retries += 1;
    }

    if retries >= 30 {
        if let Some(pid) = seekr::daemon::server::read_pid_file() {
            println!("Daemon did not stop gracefully. Killing PID {}...", pid);
            #[cfg(unix)]
            {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
        }
    } else {
        println!("Daemon stopped successfully.");
    }

    seekr::daemon::server::remove_pid_file();
    Ok(())
}

fn daemon_logs(lines: usize) -> Result<()> {
    let log_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("seekr")
        .join("seekr.log");

    if !log_path.exists() {
        println!("No log file found at {}", log_path.display());
        println!("Enable logging by setting SEEKR_LOG=1");
        return Ok(());
    }

    let content = std::fs::read_to_string(&log_path)?;
    let log_lines: Vec<&str> = content.lines().collect();
    let start = log_lines.len().saturating_sub(lines);
    for line in &log_lines[start..] {
        println!("{}", line);
    }
    Ok(())
}

fn init_logging() {
    if std::env::var("SEEKR_LOG").is_ok() {
        let log_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("seekr")
            .join("seekr.log");

        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => {
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::DEBUG)
                    .with_writer(std::sync::Mutex::new(file))
                    .init();
            }
            Err(e) => {
                eprintln!("Failed to open log file {}: {}", log_path.display(), e);
            }
        }
    }
} // init_logging

fn setup_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ =
            ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        original_hook(panic_info);
    }));
} // setup_panic_hook
