use reqwest::StatusCode;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("API request failed ({0}): {1}")]
    HttpStatus(StatusCode, String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Missing content in API response: {0}")]
    MissingContent(String),

    #[error("Invalid provider configuration: {0}")]
    InvalidProvider(String),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    Serialization(#[from] toml::ser::Error),

    #[error("Keyring error: {0}")]
    Keyring(String),

    #[error("Keyring error: {0}. Please run this command to set the key manually:\n\n  {1}")]
    KeyringWithCommand(String, String),

    #[error("Migration failed: {0}")]
    MigrationFailed(String),

    #[error("Configuration path error: {0}")]
    Path(String),
}

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Task not found: {0}")]
    TaskNotFound(usize),

    #[error("Invalid CSS selector: {0}")]
    InvalidSelector(String),

    #[error("Web fetch error: {0}")]
    WebError(String),

    #[error("Security Error: {0}")]
    SecurityError(String),

    #[error("Shell execution failed: {0}")]
    ShellExecution(String),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("File edit failed: {0}")]
    EditFailed(String),
}

#[derive(Error, Debug)]
pub enum AppError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),

    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
