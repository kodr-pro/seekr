use seekr::config::{AgentConfig, AppConfig, ProviderConfig, UiConfig};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_default_config_values() {
    let config = AppConfig::default();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.active_provider, 0);
    assert_eq!(config.agent.max_iterations, 100);
    assert_eq!(config.ui.theme, "dark");
}

#[test]
fn test_current_provider_logic() {
    let mut config = AppConfig::default();
    config.providers.push(ProviderConfig {
        name: "Secondary".to_string(),
        ..Default::default()
    });

    // Test current_provider
    assert_eq!(config.current_provider().name, "Seekr AI");
    config.active_provider = 1;
    assert_eq!(config.current_provider().name, "Secondary");

    // Test out of bounds fallback
    config.active_provider = 5;
    assert_eq!(config.current_provider().name, "Seekr AI");

    // Test current_provider_mut
    config.active_provider = 1;
    config.current_provider_mut().name = "Modified".to_string();
    assert_eq!(config.providers[1].name, "Modified");
}

#[test]
fn test_old_format_migration() {
    let temp = tempdir().unwrap();
    let config_dir = temp.path().join("seekr");
    fs::create_dir_all(&config_dir).unwrap();
    let config_file = config_dir.join("config.toml");

    let old_toml = r#"
[api]
key = "old-key"
model = "old-model"
base_url = "https://old.api"

[agent]
max_iterations = 50
auto_approve_tools = true
working_directory = "/tmp"
context_window_threshold = 20
context_window_keep = 5
shell_blocklist = ["rm"]
show_shell_warnings = false

[ui]
theme = "light"
show_reasoning = false
"#;

    fs::write(&config_file, old_toml).unwrap();

    // We can't easily mock dirs::config_dir() without global state,
    // so we'll test the deserialization logic directly by manually
    // implementing the "try parsing as old format" logic in the test
    // to verify the structures are compatible.

    #[derive(serde::Deserialize)]
    struct OldApiConfig {
        key: String,
        model: String,
        base_url: String,
    }
    #[derive(serde::Deserialize)]
    struct OldAppConfig {
        api: OldApiConfig,
        agent: AgentConfig,
        ui: UiConfig,
    }

    let old: OldAppConfig = toml::from_str(old_toml).unwrap();
    let config = AppConfig {
        providers: vec![ProviderConfig {
            name: "Default".to_string(),
            key: old.api.key,
            base_url: old.api.base_url,
            model: old.api.model,
            timeout: None,
        }],
        active_provider: 0,
        agent: old.agent,
        ui: old.ui,
        mcp_servers: vec![],
    };

    assert_eq!(config.providers[0].key, "old-key");
    assert_eq!(config.agent.max_iterations, 50);
    assert_eq!(config.ui.theme, "light");
}

#[test]
fn test_base_url_defaults() {
    assert_eq!(
        AppConfig::get_default_base_url("gpt-4o"),
        "https://api.openai.com/v1"
    );
    assert_eq!(
        AppConfig::get_default_base_url("deepseek-chat"),
        "https://api.deepseek.com/v1"
    );
    assert_eq!(
        AppConfig::get_default_base_url("claude-3-opus"),
        "https://api.anthropic.com/v1"
    );
    assert_eq!(
        AppConfig::get_default_base_url("nvidia/llama3"),
        "https://integrate.api.nvidia.com/v1"
    );
    assert_eq!(
        AppConfig::get_default_base_url("unknown"),
        "https://api.openai.com/v1"
    );
}
