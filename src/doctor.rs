// doctor.rs - Health checks and troubleshooting diagnostics
//
// Verifies configuration, API connectivity, and system environment.

use anyhow::Result;
use crate::config::AppConfig;
use crate::api::client::DeepSeekClient;
use std::path::Path;
use colored::*;

/// Summary of a doctor check result
pub enum CheckResult {
    Ok(String),
    Warning(String),
    Error(String),
}

/// Runs all diagnostic checks and prints results to stdout
pub async fn run_diagnostics() -> Result<()> {
    println!("\n{}", "Seekr Doctor 🩺".bold().cyan());
    println!("Checking your setup for any issues...\n");

    let checks = vec![
        ("Configuration", check_config()),
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

    println!("\nDiagnostics complete: {} errors, {} warnings.\n", errors, warnings);
    
    if errors > 0 {
        println!("{}", "Please fix the red issues above before running Seekr.".red());
    } else if warnings > 0 {
        println!("{}", "Seekr should run, but some features might be limited.".yellow());
    } else {
        println!("{}", "All systems go! Seekr is ready to help.".green());
    }

    Ok(())
}

fn check_config() -> CheckResult {
    if !AppConfig::exists() {
        return CheckResult::Error("Config file missing. Run Seekr to start the setup wizard.".to_string());
    }
    
    match AppConfig::load() {
        Ok(cfg) => {
            if cfg.api.key.is_empty() {
                CheckResult::Error("API key is empty in config.toml.".to_string())
            } else {
                CheckResult::Ok("Valid configuration found.".to_string())
            }
        }
        Err(e) => CheckResult::Error(format!("Failed to parse config: {}", e)),
    }
}

async fn check_api() -> CheckResult {
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => return CheckResult::Error("Cannot check API without valid config.".to_string()),
    };

    let client = DeepSeekClient::new(&cfg);
    // Simple test message to check connectivity and key validity
    // We use a very short prompt to keep it cheap/fast
    match client.chat_completion_stream(
        vec![crate::api::types::ChatMessage::user("ping")],
        &cfg.api.model,
        None,
    ).await {
        Ok(_) => CheckResult::Ok("Connected to DeepSeek API successfully.".to_string()),
        Err(e) => CheckResult::Error(format!("DeepSeek API error: {}", e)),
    }
}

fn check_working_dir() -> CheckResult {
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => return CheckResult::Error("Cannot check directory without valid config.".to_string()),
    };

    let expanded_path = shellexpand::tilde(&cfg.agent.working_directory);
    let path = Path::new(expanded_path.as_ref());
    if !path.exists() {
        return CheckResult::Error(format!("Working directory does not exist: {}", cfg.agent.working_directory));
    }
    
    if !path.is_dir() {
        return CheckResult::Error(format!("Working path is not a directory: {}", cfg.agent.working_directory));
    }

    // Check for write permissions by trying to create a temp file
    let test_file = path.join(".seekr_doctor_test");
    match std::fs::write(&test_file, "test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            CheckResult::Ok(format!("Working directory is readable and writable: {}", expanded_path))
        }
        Err(e) => CheckResult::Warning(format!("Detected limited write permissions in {}: {}", expanded_path, e)),
    }
}

fn check_system_tools() -> CheckResult {
    let mut missing = Vec::new();
    
    // Check for git
    if std::process::Command::new("git").arg("--version").output().is_err() {
        missing.push("git");
    }
    
    // Check for rustc (since it's a Rust dev tool)
    if std::process::Command::new("rustc").arg("--version").output().is_err() {
        missing.push("rustc");
    }

    if missing.is_empty() {
        CheckResult::Ok("Required system tools (git, rustc) are available.".to_string())
    } else {
        CheckResult::Warning(format!("Missing system tools: {}. Some tools might not work correctly.", missing.join(", ")))
    }
}
