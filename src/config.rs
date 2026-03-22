use anyhow::Result;
use crate::errors::ConfigError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub key: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub timeout: Option<u64>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "Seekr AI".to_string(),
            key: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
            timeout: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_iterations: u32,
    pub auto_approve_tools: bool,
    pub working_directory: String,
    pub context_window_threshold: usize,
    pub context_window_keep: usize,
    pub shell_blocklist: Vec<String>,
    pub show_shell_warnings: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            auto_approve_tools: false,
            working_directory: ".".to_string(),
            context_window_threshold: 40,
            context_window_keep: 10,
            shell_blocklist: vec![
                "rm -rf /".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                ":(){ :|:& };:".to_string(),
                "> /dev/sda".to_string(),
                "> /dev/nvme".to_string(),
                "chmod -R 777 /".to_string(),
                "chown -R".to_string(),
            ],
            show_shell_warnings: true,
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
        self.providers
            .get(self.active_provider)
            .unwrap_or_else(|| self.providers.first().expect("Config has no providers"))
    }

    pub fn current_provider_mut(&mut self) -> &mut ProviderConfig {
        if self.providers.is_empty() {
            self.providers.push(ProviderConfig::default());
        }
        let len = self.providers.len();
        if self.active_provider >= len {
            self.active_provider = 0;
        }
        &mut self.providers[self.active_provider]
    }

    pub fn config_path() -> Result<PathBuf, ConfigError> {
        let config_dir = dirs::config_dir().ok_or_else(|| ConfigError::Path("Could not determine config directory".to_string()))?;
        Ok(config_dir.join("seekr").join("config.toml"))
    } // config_path

    pub fn exists() -> bool {
        Self::config_path().map(|p| p.exists()).unwrap_or(false)
    } // exists

    pub fn load() -> Result<Self> {
        let path = Self::config_path().map_err(|e| anyhow::anyhow!(e))?;
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| ConfigError::Io(e))?;

        let mut config: AppConfig = if let Ok(config) = toml::from_str(&contents) {
            config
        } else {
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
                        timeout: None,
                    }],
                    active_provider: 0,
                    agent: old.agent,
                    ui: old.ui,
                };

                // When migrating, keys from old config will be moved to keyring on next save automatically if the user modifies anything. Or we can save immediately:
                let _ = config.save();
                config
            } else {
                return Err(ConfigError::MigrationFailed("Failed to parse config.toml - format may be corrupted".to_string()).into());
            }
        };

        // Load keys from keyring (or env override)
        for provider in &mut config.providers {
            let normalized_name = provider.name.to_lowercase().replace(" ", "_");
            let env_key = format!("SEEKR_API_KEY_{}", normalized_name.to_uppercase());

            if let Ok(env_val) = std::env::var("SEEKR_API_KEY").or_else(|_| std::env::var(&env_key))
            {
                provider.key = env_val;
            } else if provider.key.is_empty() {
                // If the key in TOML is empty, always try keyring
                let entry_name = format!("seekr_api_key_{}", normalized_name);
                match keyring::Entry::new("seekr", &entry_name) {
                    Ok(entry) => {
                        match entry.get_password() {
                            Ok(password) => {
                                if !password.is_empty() {
                                    provider.key = password;
                                }
                            }
                            Err(e) => {
                                // Only log error if it's not "No password found"
                                if !format!("{:?}", e).contains("NoEntry")
                                    && !format!("{:?}", e).contains("NotFound")
                                {
                                    tracing::warn!("Keyring error for {}: {}", entry_name, e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to initialize keyring entry for {}: {}",
                            entry_name,
                            e
                        );
                    }
                }
            }
        }

        Ok(config)
    } // load

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path().map_err(|e| anyhow::anyhow!(e))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e))?;
        }

        let mut keyring_errors = Vec::new();
        let mut saveable_config = self.clone();

        for (i, provider) in self.providers.iter().enumerate() {
            let normalized_name = provider.name.to_lowercase().replace(" ", "_");
            let entry_name = format!("seekr_api_key_{}", normalized_name);

            if !provider.key.is_empty() {
                match keyring::Entry::new("seekr", &entry_name) {
                    Ok(entry) => match entry.set_password(&provider.key) {
                        Ok(_) => {
                            saveable_config.providers[i].key = String::new();
                        }
                        Err(e) => {
                            keyring_errors.push(format!("{}: {}", entry_name, e));
                            // We DO NOT save it to TOML if it failed.
                            // The next load will simply show it as empty or use Env.
                            saveable_config.providers[i].key = String::new();
                        }
                    },
                    Err(e) => {
                        keyring_errors.push(format!("{}: {}", entry_name, e));
                        saveable_config.providers[i].key = String::new();
                    }
                }
            } else {
                saveable_config.providers[i].key = String::new();
            }
        }

        if !keyring_errors.is_empty() {
            return Err(ConfigError::Keyring(keyring_errors.join(", ")).into());
        }

        let contents =
            toml::to_string_pretty(&saveable_config).map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))?;
        std::fs::write(&path, contents)
            .map_err(|e| ConfigError::Io(e))?;
        Ok(())
    } // save

    pub fn get_default_base_url(model: &str) -> String {
        if model.contains("gpt-") {
            "https://api.openai.com/v1".to_string()
        } else if model.contains("deepseek") {
            "https://api.deepseek.com/v1".to_string()
        } else if model.contains("claude") {
            // Anthropic official API
            "https://api.anthropic.com/v1".to_string()
        } else if model.contains("nvidia/") {
            // NVIDIA NIM API
            "https://integrate.api.nvidia.com/v1".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        }
    }
} // impl AppConfig
