// main.rs - Entry point for the Seekr Agent CLI
//
// Detects first-run (no config file) and launches either the setup wizard
// or the main TUI application.

use seekr::{app, config};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for debug logging (writes to a file so it doesn't interfere with TUI)
    init_logging();

    let args: Vec<String> = std::env::args().collect();
    
    // Handle 'doctor' command
    if args.len() >= 2 && args[1] == "doctor" {
        return seekr::doctor::run_diagnostics().await;
    }

    let resume_id = if args.len() >= 3 && args[1] == "--resume" {
        Some(args[2].clone())
    } else {
        None
    };

    // First-run detection: check if config exists
    let mut app = if config::AppConfig::exists() {
        match config::AppConfig::load() {
            Ok(cfg) => app::App::new_main(cfg),
            Err(e) => {
                eprintln!(
                    "Failed to load config: {}. Starting setup wizard.",
                    e
                );
                app::App::new_setup()
            }
        }
    } else {
        app::App::new_setup()
    };

    if let Some(sid) = resume_id {
        if app.mode == app::AppMode::Main {
            app.resume_session(sid);
        }
    }

    // Run the TUI event loop
    app::run_app(app).await
}

/// Initialize tracing to a log file (non-blocking, optional)
fn init_logging() {
    // Only enable file logging if SEEKR_LOG is set
    if std::env::var("SEEKR_LOG").is_ok() {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/seekr.log")
            .expect("Failed to open log file");
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    }
}
