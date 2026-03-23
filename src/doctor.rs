use crate::api::client::ApiClient;
use crate::config::AppConfig;
use anyhow::Result;
use colored::*;
use std::path::Path;

pub enum CheckResult {
    Ok(String),
    Warning(String),
    Error(String),
}

pub async fn run_diagnostics() -> Result<()> {
    println!("\n{}", "Seekr Doctor 🩺".bold().cyan());
    println!("Checking your setup for any issues...\n");

    let checks = vec![
        ("Configuration", check_config()),
        ("Keyring Status", check_keyring()),
        ("API Connectivity", check_api().await),
        ("Working Directory", check_working_dir()),
        ("System Tools", check_system_tools()),
    ];

    let mut errors = 0;
    let mut warnings = 0;

    for (name, result) in checks {
        match result {
            CheckResult::Ok(msg) => {
                println!("  {} {} - {}", "✓".green(), name.bold(), msg);
            }
            CheckResult::Warning(msg) => {
                println!("  {} {} - {}", "!".yellow(), name.bold(), msg);
                warnings += 1;
            }
            CheckResult::Error(msg) => {
                println!("  {} {} - {}", "✗".red(), name.bold(), msg);
                errors += 1;
            }
        }
    }

    println!(
        "\nDiagnostics complete: {} errors, {} warnings.\n",
        errors, warnings
    );

    if errors > 0 {
        println!(
            "{}",
            "Please fix the red issues above before running Seekr.".red()
        );
    } else if warnings > 0 {
        println!(
            "{}",
            "Seekr should run, but some features might be limited.".yellow()
        );
    } else {
        println!("{}", "All systems go! Seekr is ready to help.".green());
    }

    Ok(())
} // run_diagnostics

fn check_config() -> CheckResult {
    if !AppConfig::exists() {
        return CheckResult::Error(
            "Config file missing. Run Seekr to start the setup wizard.".to_string(),
        );
    }

    match AppConfig::load() {
        Ok(cfg) => {
            if cfg.providers.is_empty() || cfg.current_provider().key.is_empty() {
                CheckResult::Error("No providers configured or API key is empty.".to_string())
            } else {
                CheckResult::Ok(format!(
                    "Valid configuration found for provider: {}",
                    cfg.current_provider().name
                ))
            }
        }
        Err(e) => {
            let mut msg = format!("Failed to parse config: {}", e);
            if format!("{:?}", e).contains("Keyring") {
                msg.push_str("\n    Tip: If your OS keyring is inaccessible, you can set the key via environment variable:\n    export SEEKR_API_KEY_OPENAI=your_key_here");
            }
            CheckResult::Error(msg)
        }
    }
} // check_config

async fn check_api() -> CheckResult {
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => return CheckResult::Error("Cannot check API without valid config.".to_string()),
    };

    let client = ApiClient::new(&cfg);
    let provider = cfg.current_provider();
    match client
        .chat_completion_stream(
            vec![crate::api::types::ChatMessage::user("ping")],
            &provider.model,
            None,
        )
        .await
    {
        Ok(_) => CheckResult::Ok(format!("Connected to {} API successfully.", provider.name)),
        Err(e) => CheckResult::Error(format!("{} API error: {}", provider.name, e)),
    }
} // check_api

fn check_working_dir() -> CheckResult {
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => {
            return CheckResult::Error("Cannot check directory without valid config.".to_string());
        }
    };

    let expanded_path = shellexpand::tilde(&cfg.agent.working_directory);
    let path = Path::new(expanded_path.as_ref());
    if !path.exists() {
        return CheckResult::Error(format!(
            "Working directory does not exist: {}",
            cfg.agent.working_directory
        ));
    }

    if !path.is_dir() {
        return CheckResult::Error(format!(
            "Working path is not a directory: {}",
            cfg.agent.working_directory
        ));
    }

    let test_file = path.join(".seekr_doctor_test");
    match std::fs::write(&test_file, "test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            CheckResult::Ok(format!(
                "Working directory is readable and writable: {}",
                expanded_path
            ))
        }
        Err(e) => CheckResult::Warning(format!(
            "Detected limited write permissions in {}: {}",
            expanded_path, e
        )),
    }
} // check_working_dir

fn check_keyring() -> CheckResult {
    let test_entry = "seekr_doctor_test";
    match keyring::Entry::new("seekr", test_entry) {
        Ok(entry) => match entry.set_password("test_password") {
            Ok(_) => {
                let _ = entry.delete_credential();
                CheckResult::Ok("OS Keyring is accessible and working correctly.".to_string())
            }
            Err(e) => {
                let msg = format!(
                    "Keyring found but cannot set password: {}. Your system might need a running secret service (e.g. gnome-keyring or kwallet).",
                    e
                );
                CheckResult::Error(msg)
            }
        },
        Err(e) => CheckResult::Error(format!(
            "Failed to initialize keyring: {}. Tip: You can skip the keyring by using environment variables like SEEKR_API_KEY.",
            e
        )),
    }
} // check_keyring

fn check_system_tools() -> CheckResult {
    let mut missing = Vec::new();

    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        missing.push("git");
    }

    if std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .is_err()
    {
        missing.push("rustc");
    }

    if missing.is_empty() {
        CheckResult::Ok("Required system tools (git, rustc) are available.".to_string())
    } else {
        CheckResult::Warning(format!(
            "Missing system tools: {}. Some tools might not work correctly.",
            missing.join(", ")
        ))
    }
} // check_system_tools
