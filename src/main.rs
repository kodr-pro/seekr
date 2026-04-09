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
    Daemon,
    /// Run diagnostics
    Doctor,
    /// Start TUI from a previous session
    Resume { session_id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    setup_panic_hook();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon) => {
            println!("Starting Seekr daemon...");
            return start_server().await;
        }
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

                // wait for it to boot
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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

fn init_logging() {
    if std::env::var("SEEKR_LOG").is_ok() {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/seekr.log")
        {
            Ok(file) => {
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::DEBUG)
                    .with_writer(std::sync::Mutex::new(file))
                    .init();
            }
            Err(e) => {
                eprintln!("Failed to open log file /tmp/seekr.log: {}", e);
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
