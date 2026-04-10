use crate::errors::ConfigError;
use anyhow::Result;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub auto_install: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
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
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: vec![ProviderConfig::default()],
            active_provider: 0,
            agent: AgentConfig::default(),
            ui: UiConfig::default(),
            mcp_servers: Vec::new(),
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
        let config_dir = dirs::config_dir()
            .ok_or_else(|| ConfigError::Path("Could not determine config directory".to_string()))?;
        Ok(config_dir.join("seekr").join("config.toml"))
    } // config_path

    pub fn exists() -> bool {
        Self::config_path().map(|p| p.exists()).unwrap_or(false)
    } // exists

    pub fn load() -> Result<Self> {
        let path = Self::config_path().map_err(|e| anyhow::anyhow!(e))?;
        let contents = std::fs::read_to_string(&path).map_err(ConfigError::Io)?;

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
                    mcp_servers: Vec::new(),
                };

                // When migrating, keys from old config will be moved to keyring on next save automatically if the user modifies anything. Or we can save immediately:
                let _ = config.save();
                config
            } else {
                return Err(ConfigError::MigrationFailed(
                    "Failed to parse config.toml - format may be corrupted".to_string(),
                )
                .into());
            }
        };

        // Load keys from environment or keyring (legacy fallback)
        for provider in &mut config.providers {
            // 1. Check Standard Env Var: [PROVIDER]_API_KEY (e.g., DEEPSEEK_API_KEY)
            let std_env_key = format!(
                "{}_API_KEY",
                provider
                    .name
                    .to_uppercase()
                    .replace(" ", "_")
                    .replace("-", "_")
            );
            if let Some(val) = std::env::var(&std_env_key)
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                tracing::debug!(
                    "Using environment variable {} for provider: {}",
                    std_env_key,
                    provider.name
                );
                provider.key = val;
                continue;
            }

            // 2. Check Generic Env Var: SEEKR_API_KEY (Legacy/Override)
            if let Some(val) = std::env::var("SEEKR_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                tracing::debug!(
                    "Using environment variable SEEKR_API_KEY for provider: {}",
                    provider.name
                );
                provider.key = val;
                continue;
            }

            // 3. Fallback to Keyring ONLY if key is still empty (Legacy Support)
            if provider.key.is_empty() {
                let normalized_name = provider
                    .name
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect::<String>();
                let entry_name = format!("seekr_api_key_{}", normalized_name);

                if let Some(password) = keyring::Entry::new("seekr", &entry_name)
                    .and_then(|entry| entry.get_password())
                    .ok()
                    .filter(|p| !p.trim().is_empty())
                {
                    provider.key = password;
                    tracing::debug!("Loaded legacy key from keyring for {}", entry_name);
                }
            }
        }

        Ok(config)
    } // load

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path().map_err(|e| anyhow::anyhow!(e))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Io)?;
        }

        let contents = toml::to_string_pretty(self).map_err(ConfigError::Serialization)?;
        std::fs::write(&path, contents).map_err(ConfigError::Io)?;

        // Set file permissions to 600 (read/write by owner only) on Unix-like systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)
                .map_err(ConfigError::Io)?
                .permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&path, perms).map_err(ConfigError::Io)?;
        }

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
        } else if model.contains("gemini") {
            "https://generativelanguage.googleapis.com/v1beta/openai/".to_string()
        } else if model.contains("nvidia/") {
            // NVIDIA NIM API
            "https://integrate.api.nvidia.com/v1".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        }
    }
} // impl AppConfig
