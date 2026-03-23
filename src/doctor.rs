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
        ("File Security", check_file_permissions()),
        ("Keyring (Legacy)", check_keyring_legacy().await),
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

    println!("{}", "Tip: You can add more AI providers or edit existing ones anytime using Ctrl+G in the main menu.".dimmed());

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
            let mut results = Vec::new();
            for provider in &cfg.providers {
                if provider.key.is_empty() {
                    let env_key = format!(
                        "{}_API_KEY",
                        provider
                            .name
                            .to_uppercase()
                            .replace(" ", "_")
                            .replace("-", "_")
                    );
                    results.push(format!(
                        "✗ {} (Missing key - checked TOML and {})",
                        provider.name, env_key
                    ));
                } else {
                    results.push(format!("✓ {}", provider.name));
                }
            }

            if results.iter().any(|r| r.contains("✗")) {
                CheckResult::Error(format!(
                    "Provider configurations:\n      {}",
                    results.join("\n      ")
                ))
            } else {
                CheckResult::Ok(format!(
                    "All {} providers configured correctly.",
                    cfg.providers.len()
                ))
            }
        }
        Err(e) => {
            let mut msg = format!("Failed to load config: {}", e);
            if format!("{:?}", e).contains("Keyring") {
                msg.push_str("\n    Tip: If your OS keyring is inaccessible, you can set the key in config.toml or via environment variable.");
            }
            CheckResult::Error(msg)
        }
    }
} // check_config

fn check_file_permissions() -> CheckResult {
    let path = match AppConfig::config_path() {
        Ok(p) => p,
        Err(_) => return CheckResult::Error("Cannot find config path.".to_string()),
    };

    if !path.exists() {
        return CheckResult::Ok("Config file does not exist yet.".to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(&path) {
            Ok(meta) => {
                let mode = meta.permissions().mode() & 0o777;
                if mode == 0o600 {
                    CheckResult::Ok("Config file has secure permissions (600).".to_string())
                } else {
                    CheckResult::Warning(format!(
                        "Config file has loose permissions ({:o}). Recommendation: chmod 600 {:?}",
                        mode, path
                    ))
                }
            }
            Err(e) => CheckResult::Error(format!("Failed to read config file metadata: {}", e)),
        }
    }
    #[cfg(not(unix))]
    {
        CheckResult::Ok("File permissions check skipped on non-Unix system.".to_string())
    }
} // check_file_permissions

async fn check_keyring_legacy() -> CheckResult {
    // This is now considered a legacy check
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => return CheckResult::Error("Cannot load config for keyring check.".to_string()),
    };

    let mut results = Vec::new();
    for provider in &cfg.providers {
        if provider.key.is_empty() {
            results.push(format!("! {} (Skipped - no key)", provider.name));
            continue;
        }

        let normalized_name = provider
            .name
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>();
        let entry_name = format!("seekr_api_key_{}", normalized_name);

        match keyring::Entry::new("seekr", &entry_name) {
            Ok(entry) => match entry.get_password() {
                Ok(p) => {
                    if p.is_empty() {
                        results.push(format!("Entry '{}': FOUND but EMPTY.", entry_name));
                    } else {
                        results.push(format!(
                            "Entry '{}': FOUND and has data ({} chars).",
                            entry_name,
                            p.len()
                        ));
                    }
                }
                Err(e) => {
                    results.push(format!(
                        "Entry '{}': NOT FOUND or inaccessible ({}).",
                        entry_name, e
                    ));
                }
            },
            Err(e) => results.push(format!("Entry '{}': Init failed ({}).", entry_name, e)),
        }
    }

    if results
        .iter()
        .any(|d| d.contains("FAILED") || d.contains("NOT FOUND"))
    {
        CheckResult::Warning(results.join("\n      "))
    } else {
        CheckResult::Ok(results.join("\n      "))
    }
} // check_keyring_legacy

async fn check_api() -> CheckResult {
    let cfg = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(_) => return CheckResult::Error("Cannot check API without valid config.".to_string()),
    };

    let mut results = Vec::new();
    for provider in &cfg.providers {
        if provider.key.is_empty() {
            results.push(format!("! {} (Skipped - no key)", provider.name));
            continue;
        }

        let client = ApiClient::new_for_provider(&cfg, provider);
        match client
            .chat_completion_stream(
                vec![crate::api::types::ChatMessage::user("ping")],
                &provider.model,
                None,
            )
            .await
        {
            Ok(_) => results.push(format!("✓ {} (Connected)", provider.name)),
            Err(e) => results.push(format!("✗ {} (Error: {})", provider.name, e)),
        }
    }

    if results.iter().any(|r| r.contains("✗")) {
        CheckResult::Error(format!(
            "API Connectivity:\n      {}",
            results.join("\n      ")
        ))
    } else if results.iter().any(|r| r.contains("!")) {
        CheckResult::Warning(format!(
            "API Connectivity:\n      {}",
            results.join("\n      ")
        ))
    } else {
        CheckResult::Ok("All providers connected successfully.".to_string())
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
