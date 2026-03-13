// main.rs - Entry point for the Seekr Agent CLI
//
// Detects first-run (no config file) and launches either the setup wizard
// or the main TUI application.

mod agent;
mod api;
mod app;
mod config;
mod session;
mod tools;
mod ui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for debug logging (writes to a file so it doesn't interfere with TUI)
    init_logging();

    // First-run detection: check if config exists
    let app = if config::AppConfig::exists() {
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
