use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::questions::QuestionSet;
use super::JevValue;

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const DEFAULT_MODEL: &str = "jev-1.13.0";
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Error, Debug)]
pub enum JevError {
    #[error("jev egress is disabled (JEV_EGRESS=off)")]
    EgressOff,
    #[error("jev API key missing (set TYPESAFE_API_KEY)")]
    NoApiKey,
    #[error("jev request failed ({0}): {1}")]
    HttpStatus(StatusCode, String),
    #[error("jev HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("jev response JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid jev response: {0}")]
    InvalidResponse(String),
    #[error("jev cache I/O error: {0}")]
    CacheIo(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    EgressOff,
    NoApiKey,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JevUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JevResult {
    pub model: String,
    pub answers: BTreeMap<String, JevValue>,
    pub usage: JevUsage,
    pub cached: bool,
}

#[derive(Clone, Debug)]
pub struct JevConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub egress_enabled: bool,
    pub timeout: Duration,
    pub max_retries: u32,
    pub cache_dir: Option<PathBuf>,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            egress_enabled: true,
            timeout: Duration::from_secs(60),
            max_retries: 2,
            cache_dir: dirs::cache_dir().map(|d| d.join("seekr").join("jev")),
        }
    }
}

impl JevConfig {
    /// Config from environment: `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`,
    /// `JEV_MODEL`, and `JEV_EGRESS` (only "1/true/on/yes" enables; anything
    /// else, including "off", disables — fail-closed).
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(key) = std::env::var("TYPESAFE_API_KEY") {
            cfg.api_key = key.trim().to_string();
        }
        if let Ok(url) = std::env::var("TYPESAFE_BASE_URL") {
            if !url.trim().is_empty() {
                cfg.base_url = url.trim().trim_end_matches('/').to_string();
            }
        }
        if let Ok(model) = std::env::var("JEV_MODEL") {
            if !model.trim().is_empty() {
                cfg.model = model.trim().to_string();
            }
        }
        if let Ok(egress) = std::env::var("JEV_EGRESS") {
            cfg.egress_enabled = matches!(
                egress.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            );
        }
        cfg
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WireResponse {
    model: String,
    answers: BTreeMap<String, JevValue>,
    usage: WireUsage,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WireUsage {
    input_tokens: u64,
    output_tokens: u64,
}

/// Client for the TypeSafe AI System One endpoint (`POST /v1/systemone`).
/// One `ask` = one batched round trip covering every question in the set.
#[derive(Clone)]
pub struct JevClient {
    http: Client,
    config: JevConfig,
}

impl JevClient {
    pub fn new(config: JevConfig) -> Self {
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_default();
        Self { http, config }
    }

    pub fn from_env() -> Self {
        Self::new(JevConfig::from_env())
    }

    pub fn config(&self) -> &JevConfig {
        &self.config
    }

    /// Fail-closed availability check. Returns `Some(reason)` when semantic
    /// judgments must not be attempted.
    pub fn unavailable(&self) -> Option<UnavailableReason> {
        if !self.config.egress_enabled {
            Some(UnavailableReason::EgressOff)
        } else if self.config.api_key.is_empty() {
            Some(UnavailableReason::NoApiKey)
        } else {
            None
        }
    }

    fn cache_key(&self, state: &Value, questions: &QuestionSet) -> String {
        let canonical = serde_json::json!({
            "model": self.config.model,
            "questions": questions.questions,
            "state": state,
        });
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }

    fn cache_path(&self, key: &str) -> Option<PathBuf> {
        self.config
            .cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("{key}.json")))
    }

    fn cache_load(&self, key: &str) -> Option<JevResult> {
        let path = self.cache_path(key)?;
        let raw = std::fs::read_to_string(path).ok()?;
        let mut result: JevResult = serde_json::from_str(&raw).ok()?;
        result.cached = true;
        Some(result)
    }

    fn cache_store(&self, key: &str, result: &JevResult) {
        if let Some(dir) = self.config.cache_dir.as_ref() {
            if std::fs::create_dir_all(dir).is_ok() {
                if let Ok(body) = serde_json::to_string(result) {
                    let _ = std::fs::write(dir.join(format!("{key}.json")), body);
                }
            }
        }
    }

    /// Asks the full question set in a single batched call. Cache-first:
    /// identical (state, questions, model) never re-bills.
    pub async fn ask(&self, state: &Value, questions: &QuestionSet) -> Result<JevResult, JevError> {
        if questions.is_empty() {
            return Err(JevError::InvalidResponse(
                "question set must not be empty".to_string(),
            ));
        }
        if let Some(reason) = self.unavailable() {
            return Err(match reason {
                UnavailableReason::EgressOff => JevError::EgressOff,
                UnavailableReason::NoApiKey => JevError::NoApiKey,
            });
        }

        let key = self.cache_key(state, questions);
        if let Some(hit) = self.cache_load(&key) {
            return Ok(hit);
        }

        let body = serde_json::json!({
            "state": state,
            "questions": questions.questions,
            "model": self.config.model,
        });
        let url = format!("{}/v1/systemone", self.config.base_url);

        let response = self.send_with_retry(&url, &body).await?;
        let wire: WireResponse = response.json().await?;

        let mut result = JevResult {
            model: wire.model,
            answers: wire.answers,
            usage: JevUsage {
                input_tokens: wire.usage.input_tokens,
                output_tokens: wire.usage.output_tokens,
            },
            cached: false,
        };
        for (name, value) in result.answers.iter_mut() {
            value.sanitize().map_err(|e| {
                JevError::InvalidResponse(format!("answer '{name}' failed validation: {e}"))
            })?;
        }
        if let Some(missing) = questions
            .questions
            .keys()
            .find(|k| !result.answers.contains_key(*k))
        {
            return Err(JevError::InvalidResponse(format!(
                "missing answer for question '{missing}'"
            )));
        }

        self.cache_store(&key, &result);
        Ok(result)
    }

    async fn send_with_retry(&self, url: &str, body: &Value) -> Result<reqwest::Response, JevError> {
        let mut last_err = None;
        for attempt in 0..=self.config.max_retries {
            let result = self
                .http
                .post(url)
                .bearer_auth(&self.config.api_key)
                .json(body)
                .send()
                .await;

            match result {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    if status == StatusCode::TOO_MANY_REQUESTS
                        || status == StatusCode::REQUEST_TIMEOUT
                        || status.is_server_error()
                    {
                        let retry_after = response
                            .headers()
                            .get("retry-after-ms")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse::<u64>().ok())
                            .map(Duration::from_millis)
                            .or_else(|| {
                                response
                                    .headers()
                                    .get("Retry-After")
                                    .and_then(|v| v.to_str().ok())
                                    .and_then(|v| v.trim().parse::<u64>().ok())
                                    .map(Duration::from_secs)
                            })
                            .map(|d| d.min(MAX_RETRY_AFTER));
                        let error_body = response.text().await.unwrap_or_default();
                        last_err =
                            Some(JevError::HttpStatus(status, error_body));
                        if attempt == self.config.max_retries {
                            break;
                        }
                        let base = self.backoff_delay(attempt);
                        let delay = retry_after.unwrap_or(base).min(MAX_RETRY_AFTER);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    let error_body = response.text().await.unwrap_or_default();
                    return Err(JevError::HttpStatus(status, error_body));
                }
                Err(e) => {
                    last_err = Some(JevError::Http(e));
                }
            }
            if attempt == self.config.max_retries {
                break;
            }
            tokio::time::sleep(self.backoff_delay(attempt)).await;
        }
        Err(last_err.unwrap_or_else(|| JevError::InvalidResponse("retry loop exhausted".into())))
    }

    fn backoff_delay(&self, attempt: u32) -> Duration {
        let base_ms = 500u64.saturating_mul(2u64.saturating_pow(attempt)).min(5000);
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() % 250)
            .unwrap_or(0);
        Duration::from_millis(base_ms + jitter as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::questions::{choice, instructions, noul, score};
    use serde_json::json;

    fn sample_set() -> QuestionSet {
        let mut criteria = BTreeMap::new();
        criteria.insert("syntax_fix".to_string(), json!("compiler error"));
        criteria.insert("deadlock".to_string(), json!("architectural block"));
        QuestionSet::new()
            .add(
                "scope",
                noul(instructions("In scope?", "Step governs."), None, None),
            )
            .add("triage", choice(instructions("Pick.", "One."), criteria))
            .add(
                "novelty",
                score(
                    instructions("Novel?", "vs failures"),
                    vec![json!("identical"), json!("novel")],
                )
                .unwrap(),
            )
    }

    #[test]
    fn cache_key_is_deterministic_and_order_sensitive() {
        let cfg = JevConfig::default();
        let client = JevClient::new(cfg);
        let state = json!({"task": "fix the bug"});
        let set = sample_set();
        let k1 = client.cache_key(&state, &set);
        let k2 = client.cache_key(&state, &set);
        assert_eq!(k1, k2);
        let other = client.cache_key(&json!({"task": "other"}), &set);
        assert_ne!(k1, other);
    }

    #[test]
    fn wire_response_parses_all_three_types() {
        let raw = r#"{
            "model": "jev-1.13.0",
            "answers": {
                "scope": {"type": "noul", "noul": 0.91},
                "triage": {"type": "choice", "choice": "syntax_fix", "confidence": 0.82,
                           "probabilities": {"syntax_fix": 0.82, "deadlock": 0.18}},
                "novelty": {"type": "score", "score": 2.4, "confidence": 0.7,
                            "legend": {"0": "identical", "1": "novel"},
                            "probabilities": {"0": 0.2, "1": 0.8}}
            },
            "usage": {"input_tokens": 949, "output_tokens": 83}
        }"#;
        let wire: WireResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(wire.model, "jev-1.13.0");
        assert_eq!(wire.answers["scope"].as_noul(), Some(0.91));
        assert_eq!(wire.usage.input_tokens, 949);
    }

    #[test]
    fn unavailable_is_fail_closed() {
        let mut cfg = JevConfig::default();
        assert_eq!(cfg.egress_enabled, true);
        cfg.egress_enabled = false;
        let client = JevClient::new(cfg);
        assert_eq!(client.unavailable(), Some(UnavailableReason::EgressOff));

        let cfg = JevConfig::default();
        let client = JevClient::new(cfg);
        assert_eq!(client.unavailable(), Some(UnavailableReason::NoApiKey));
    }

    #[tokio::test]
    async fn ask_rejects_empty_question_set_and_unavailable() {
        let mut cfg = JevConfig::default();
        cfg.api_key = "ts-test".to_string();
        let client = JevClient::new(cfg);
        let err = client
            .ask(&json!({}), &QuestionSet::new())
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::InvalidResponse(_)));

        let mut cfg = JevConfig::default();
        cfg.api_key = "ts-test".to_string();
        cfg.egress_enabled = false;
        let client = JevClient::new(cfg);
        let err = client.ask(&json!({}), &sample_set()).await.unwrap_err();
        assert!(matches!(err, JevError::EgressOff));
    }

    #[test]
    fn egress_env_is_fail_closed() {
        let vars = ["JEV_EGRESS", "TYPESAFE_API_KEY", "TYPESAFE_BASE_URL", "JEV_MODEL"];
        let saved: Vec<(String, Result<String, std::env::VarError>)> = vars
            .iter()
            .map(|v| (v.to_string(), std::env::var(v)))
            .collect();
        unsafe { std::env::set_var("JEV_EGRESS", "off") };
        let cfg = JevConfig::from_env();
        assert!(!cfg.egress_enabled);
        unsafe { std::env::set_var("JEV_EGRESS", "ON") };
        let cfg = JevConfig::from_env();
        assert!(cfg.egress_enabled);
        unsafe { std::env::remove_var("JEV_EGRESS") };
        let cfg = JevConfig::from_env();
        assert!(cfg.egress_enabled);
        for (var, val) in saved {
            match val {
                Ok(v) => unsafe { std::env::set_var(&var, v) },
                Err(_) => unsafe { std::env::remove_var(&var) },
            }
        }
    }
}
