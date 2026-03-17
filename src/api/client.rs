use anyhow::{Context, Result};
use reqwest::Client;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use super::stream::{parse_sse_stream, StreamEvent};
use super::types::*;

#[derive(Clone)]
pub struct ApiClient {
    http: Client,
    base_url: String,
    api_key: String,
}

impl ApiClient {
    pub fn new(config: &AppConfig) -> Self {
        let provider = config.current_provider();
        Self {
            http: Client::new(),
            base_url: provider.base_url.clone(),
            api_key: provider.key.clone(),
        }
    } // new

    pub async fn chat_completion_stream(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>> {
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
            tool_choice: Some(serde_json::json!("auto")),
        };

        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
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

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            if let Err(e) = parse_sse_stream(response, tx.clone()).await {
                let _ = tx.send(StreamEvent::Error(format!("Stream parse error: {e}")));
            }
        });

        Ok(rx)
    } // chat_completion_stream

    pub async fn chat_completion(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
    ) -> Result<String> {
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

        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
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

        let result: serde_json::Value = response.json().await.context("Failed to parse JSON response")?;
        
        if let Some(content) = result["choices"][0]["message"]["content"].as_str() {
            Ok(content.to_string())
        } else {
            anyhow::bail!("No content in API response")
        }
    } // chat_completion

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch models from API")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch models: {}", response.status());
        }

        let list: ModelList = response.json().await.context("Failed to parse model list")?;
        Ok(list.data.into_iter().map(|m| m.id).collect())
    } // list_models


    pub async fn validate_key(api_key: &str, base_url: &str, model: &str) -> Result<bool> {
        let client = Client::new();
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

        let url = format!("{}/chat/completions", base_url);
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to connect to API")?;

        Ok(response.status().is_success())
    } // validate_key
} // impl ApiClient
