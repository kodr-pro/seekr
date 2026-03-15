use anyhow::{Context, Result};
use reqwest::Client;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use super::stream::{parse_sse_stream, StreamEvent};
use super::types::*;

pub struct DeepSeekClient {
    http: Client,
    base_url: String,
    api_key: String,
}

impl DeepSeekClient {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            http: Client::new(),
            base_url: config.api.base_url.clone(),
            api_key: config.api.key.clone(),
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
            .context("Failed to send request to DeepSeek API")?;

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


    pub async fn validate_key(api_key: &str, base_url: &str) -> Result<bool> {
        let client = Client::new();
        let request = ChatCompletionRequest {
            model: "deepseek-chat".to_string(),
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
            .context("Failed to connect to DeepSeek API")?;

        Ok(response.status().is_success())
    } // validate_key
} // impl DeepSeekClient
