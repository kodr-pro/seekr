use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

use super::anthropic::AnthropicProvider;
use super::openai::OpenAiProvider;
use super::provider::Provider;
use super::stream::StreamEvent;
use super::types::*;
use crate::config::AppConfig;

use std::sync::Arc;

/// A client for communicating with various AI providers (OpenAI, Anthropic).
/// Handles both streaming and non-streaming chat completions.
#[derive(Clone)]
pub struct ApiClient {
    http: Client,
    base_url: String,
    api_key: String,
    provider: Arc<dyn Provider>,
}

impl ApiClient {
    /// Creates a new `ApiClient` from the given application configuration.
    pub fn new(config: &AppConfig) -> Self {
        let provider_cfg = config.current_provider();
        let mut client_builder = Client::builder();
        if let Some(timeout_secs) = provider_cfg.timeout {
            client_builder = client_builder.timeout(Duration::from_secs(timeout_secs));
        }
        let http = client_builder.build().unwrap_or_else(|_| Client::new());

        // Determine provider implementation based on base_url or explicit config
        let provider: Arc<dyn Provider> = if provider_cfg.base_url.contains("anthropic.com") {
            Arc::new(AnthropicProvider)
        } else {
            Arc::new(OpenAiProvider)
        };

        Self {
            http,
            base_url: provider_cfg.base_url.clone(),
            api_key: provider_cfg.key.clone(),
            provider,
        }
    }

    async fn send_request_with_retry<F>(&self, mut request_builder: F) -> Result<reqwest::Response>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 500;
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            match request_builder().send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(response);
                    }
                    let status = response.status();
                    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                        let error_body = response.text().await.unwrap_or_default();
                        last_error = Some(anyhow::anyhow!("HTTP {}: {}", status, error_body));
                    } else {
                        let body = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Failed to read error body".to_string());
                        anyhow::bail!("API request failed ({}): {}", status, body);
                    }
                }
                Err(e) => {
                    last_error = Some(e.into());
                }
            }
            if attempt < MAX_RETRIES {
                let delay = BASE_DELAY_MS * 2u64.pow(attempt);
                sleep(Duration::from_millis(delay)).await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error after retries")))
    }

    /// Sends a chat completion request and returns a receiver for the stream of events.
    pub async fn chat_completion_stream(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>> {
        let is_anthropic = self.provider.name() == "Anthropic";

        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages,
            temperature: Some(1.0),
            max_tokens: Some(4096),
            top_p: None,
            stream: true,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            response_format: None,
            tools,
            tool_choice: None,
        };

        let request_body = self.provider.format_request(&request);
        let url = if is_anthropic {
            format!("{}/messages", self.base_url)
        } else {
            format!("{}/chat/completions", self.base_url)
        };
        let headers = self.provider.auth_headers(&self.api_key);

        let response = self
            .send_request_with_retry(|| {
                self.http
                    .post(&url)
                    .headers(headers.clone())
                    .json(&request_body)
            })
            .await?;

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            if is_anthropic {
                if let Err(e) =
                    crate::api::stream::parse_anthropic_sse_stream(response, tx.clone()).await
                {
                    let _ = tx.send(StreamEvent::Error(format!(
                        "Anthropic Stream parse error: {e}"
                    )));
                }
            } else {
                if let Err(e) = crate::api::stream::parse_sse_stream(response, tx.clone()).await {
                    let _ = tx.send(StreamEvent::Error(format!(
                        "OpenAI Stream parse error: {e}"
                    )));
                }
            }
        });

        Ok(rx)
    } // chat_completion_stream

    /// Sends a non-streaming chat completion request and returns the full response content.
    pub async fn chat_completion(&self, messages: Vec<ChatMessage>, model: &str) -> Result<String> {
        let is_anthropic = self.provider.name() == "Anthropic";

        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages,
            temperature: Some(0.7),
            max_tokens: Some(4096),
            top_p: None,
            stream: false,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            response_format: None,
            tools: None,
            tool_choice: None,
        };

        let request_body = self.provider.format_request(&request);
        let headers = self.provider.auth_headers(&self.api_key);

        let url = if is_anthropic {
            format!("{}/messages", self.base_url)
        } else {
            format!("{}/chat/completions", self.base_url)
        };

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            anyhow::bail!("API request failed ({}): {}", status, body);
        }

        let result: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse JSON response")?;

        if is_anthropic {
            if let Some(content_array) = result["content"].as_array() {
                for content_block in content_array {
                    if content_block["type"] == "text" {
                        if let Some(text) = content_block["text"].as_str() {
                            return Ok(text.to_string());
                        }
                    }
                }
            }
            anyhow::bail!("No text content in Anthropic API response")
        } else {
            if let Some(content) = result["choices"][0]["message"]["content"].as_str() {
                Ok(content.to_string())
            } else {
                anyhow::bail!("No content in API response")
            }
        }
    } // chat_completion

    /// Retrieves a list of available models from the provider.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let is_anthropic = self.provider.name() == "Anthropic";

        if is_anthropic {
            Ok(vec![
                "claude-3-5-sonnet-20241022".to_string(),
                "claude-3-5-haiku-20241022".to_string(),
                "claude-3-opus-20240229".to_string(),
                "claude-3-sonnet-20240229".to_string(),
                "claude-3-haiku-20240307".to_string(),
                "claude-2.1".to_string(),
                "claude-2.0".to_string(),
                "claude-instant-1.2".to_string(),
            ])
        } else {
            let url = format!("{}/models", self.base_url);
            let headers = self.provider.auth_headers(&self.api_key);
            let response = self
                .http
                .get(&url)
                .headers(headers)
                .send()
                .await
                .context("Failed to fetch models from API")?;

            if !response.status().is_success() {
                anyhow::bail!("Failed to fetch models: {}", response.status());
            }

            let list: ModelList = response
                .json()
                .await
                .context("Failed to parse model list")?;
            Ok(list.data.into_iter().map(|m| m.id).collect())
        }
    } // list_models

    /// Tests the validity of an API key and base URL by sending a minimal test request.
    pub async fn validate_key(api_key: &str, base_url: &str, model: &str) -> Result<bool> {
        let client = Client::new();
        let is_anthropic = base_url.contains("anthropic.com");

        let provider: Arc<dyn Provider> = if is_anthropic {
            Arc::new(AnthropicProvider)
        } else {
            Arc::new(OpenAiProvider)
        };

        let headers = provider.auth_headers(api_key);

        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![ChatMessage::user("Hi")],
            temperature: Some(1.0),
            max_tokens: Some(8),
            top_p: None,
            stream: false,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            response_format: None,
            tools: None,
            tool_choice: None,
        };

        let request_body = provider.format_request(&request);

        let url = if is_anthropic {
            format!("{}/messages", base_url)
        } else {
            format!("{}/chat/completions", base_url)
        };

        let response = client
            .post(&url)
            .headers(headers)
            .json(&request_body)
            .send()
            .await
            .context("Failed to connect to API")?;

        Ok(response.status().is_success())
    } // validate_key
} // impl ApiClient

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_messages_with_null_content() {
        // Create a message with null content
        let msg = ChatMessage {
            role: "user".to_string(),
            content: None,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        };

        // This should not panic when processing
        // We can't test the private conversion logic directly,
        // but we can at least ensure the struct works
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, None);
    }
}
