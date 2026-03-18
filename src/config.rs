use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub key: String,
    pub base_url: String,
    pub model: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "Seekr AI".to_string(),
            key: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
        }
    }
}

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
} // default

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
} // default

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    pub active_provider: usize,
    pub agent: AgentConfig,
    pub ui: UiConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: vec![ProviderConfig::default()],
            active_provider: 0,
            agent: AgentConfig::default(),
            ui: UiConfig::default(),
        }
    }
} // default

impl AppConfig {
    pub fn current_provider(&self) -> &ProviderConfig {
        &self.providers[self.active_provider]
    }

    pub fn current_provider_mut(&mut self) -> &mut ProviderConfig {
        &mut self.providers[self.active_provider]
    }

    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?;
        Ok(config_dir.join("seekr").join("config.toml"))
    } // config_path

    pub fn exists() -> bool {
        Self::config_path().map(|p| p.exists()).unwrap_or(false)
    } // exists

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let contents = std::fs::read_to_string(&path).with_context(|| {
            format!("Failed to read config from {}", path.display())
        })?;

        // Handle migration from old format
        if let Ok(config) = toml::from_str::<AppConfig>(&contents) {
            return Ok(config);
        }

        // Try parsing as old format
        #[derive(Deserialize)]
        struct OldApiConfig {
            key: String,
            model: String,
            base_url: String,
        }
        #[derive(Deserialize)]
        struct OldAppConfig {
            api: OldApiConfig,
            agent: AgentConfig,
            ui: UiConfig,
        }

        if let Ok(old) = toml::from_str::<OldAppConfig>(&contents) {
            let config = AppConfig {
                providers: vec![ProviderConfig {
                    name: "Default".to_string(),
                    key: old.api.key,
                    base_url: old.api.base_url,
                    model: old.api.model,
                }],
                active_provider: 0,
                agent: old.agent,
                ui: old.ui,
            };
            // Save migrated config
            let _ = config.save();
            return Ok(config);
        }

        anyhow::bail!("Failed to parse config.toml - format may be corrupted");
    } // load

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
    } // save

    pub fn get_default_base_url(model: &str) -> String {
        if model.contains("gpt-") {
            "https://api.openai.com/v1".to_string()
        } else if model.contains("deepseek") {
            "https://api.deepseek.com/v1".to_string()
        } else if model.contains("claude") {
            // Claude via OpenAI-compatible proxy (like OpenRouter or similar)
            "https://api.openai.com/v1".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        }
    }
} // impl AppConfig
