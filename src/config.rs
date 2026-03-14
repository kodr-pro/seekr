// config.rs - Configuration loading, saving, and defaults
//
// Manages the application configuration stored at ~/.config/seekr/config.toml.
// Provides first-run detection and the data structures for the setup wizard.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// API configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub key: String,
    pub model: String,
    pub base_url: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            key: String::new(),
            model: "deepseek-chat".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
        }
    }
}

/// Agent behavior configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_iterations: u32,
    pub auto_approve_tools: bool,
    pub working_directory: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            auto_approve_tools: false,
            working_directory: ".".to_string(),
        }
    }
}

/// UI configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: String,
    pub show_reasoning: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            show_reasoning: true,
        }
    }
}

/// Top-level application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api: ApiConfig,
    pub agent: AgentConfig,
    pub ui: UiConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            agent: AgentConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl AppConfig {
    /// Returns the path to the config file: ~/.config/seekr/config.toml
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?;
        Ok(config_dir.join("seekr").join("config.toml"))
    }

    /// Check if a config file already exists (first-run detection)
    pub fn exists() -> bool {
        Self::config_path().map(|p| p.exists()).unwrap_or(false)
    }

    /// Load configuration from disk
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!("Failed to read config from {}", path.display())
        })?;
        let config: AppConfig = toml::from_str(&contents)
            .with_context(|| "Failed to parse config.toml")?;
        Ok(config)
    }

    /// Save configuration to disk, creating directories as needed
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create config directory: {}",
                    parent.display()
                )
            })?;
        }
        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        std::fs::write(&path, contents).with_context(|| {
            format!("Failed to write config to {}", path.display())
        })?;
        Ok(())
    }
}
