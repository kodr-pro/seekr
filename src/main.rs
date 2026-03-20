use seekr::{app, config};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    setup_panic_hook();

    let args: Vec<String> = std::env::args().collect();
    
    if args.len() >= 2 && args[1] == "doctor" {
        return seekr::doctor::run_diagnostics().await;
    }

    let resume_id = if args.len() >= 3 && args[1] == "--resume" {
        Some(args[2].clone())
    } else {
        None
    };

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

    app::run_app(app).await
} // main

fn init_logging() {
    if std::env::var("SEEKR_LOG").is_ok() {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/seekr.log") {
                Ok(file) => {
                    tracing_subscriber::fmt()
                        .with_max_level(tracing::Level::DEBUG)
                        .with_writer(std::sync::Mutex::new(file))
                        .init();
                },
                Err(e) => {
                    eprintln!("Failed to open log file /tmp/seekr.log: {}", e);
                }
            }
    }
} // init_logging

fn setup_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        original_hook(panic_info);
    }));
} // setup_panic_hook
