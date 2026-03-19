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
        let is_anthropic = self.base_url.contains("anthropic.com");
        
        if is_anthropic {
            // Anthropic doesn't support streaming in the same way, and tool support is different.
            // We'll do a non‑streaming request and emit the result as a single delta.
            let url = format!("{}/messages", self.base_url);
            
            // Separate system messages and convert messages
            let mut system_prompt = String::new();
            let mut anthropic_messages = Vec::new();
            
            for msg in messages {
                if msg.role == "system" {
                    if let Some(content) = msg.content {
                        if !system_prompt.is_empty() {
                            system_prompt.push('\n');
                        }
                        system_prompt.push_str(&content);
                    }
                } else {
                    if let Some(content) = msg.content {
                        anthropic_messages.push(serde_json::json!({
                            "role": msg.role,
                            "content": content
                        }));
                    }
                }
            }
            
            let mut request_body = serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "messages": anthropic_messages,
                "stream": false,
                "temperature": 1.0,
            });
            
            if !system_prompt.is_empty() {
                request_body["system"] = serde_json::Value::String(system_prompt);
            }
            
            let response = self
                .http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .context("Failed to send request to Anthropic API")?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read error body".to_string());
                anyhow::bail!("API request failed ({}): {}", status, body);
            }

            let result: serde_json::Value = response.json().await.context("Failed to parse Anthropic JSON response")?;
            
            let (tx, rx) = mpsc::unbounded_channel();
            
            tokio::spawn(async move {
                // Extract text from Anthropic response
                if let Some(content_array) = result["content"].as_array() {
                    for content_block in content_array {
                        if content_block["type"] == "text" {
                            if let Some(text) = content_block["text"].as_str() {
                                let _ = tx.send(StreamEvent::ContentDelta(text.to_string()));
                                // Send a dummy usage event (we don't have token counts)
                                let _ = tx.send(StreamEvent::Usage {
                                    prompt_tokens: 0,
                                    completion_tokens: 0,
                                    total_tokens: 0,
                                });
                                let _ = tx.send(StreamEvent::Done);
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(StreamEvent::Error("No text content in Anthropic response".to_string()));
            });
            
            Ok(rx)
        } else {
            // Original OpenAI‑compatible streaming logic
            let tool_choice = tools.as_ref().map(|_| serde_json::json!("auto"));
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
                tool_choice,
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
        }
    } // chat_completion_stream

    pub async fn chat_completion(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
    ) -> Result<String> {
        let is_anthropic = self.base_url.contains("anthropic.com");
        
        if is_anthropic {
            // Anthropic API format
            let url = format!("{}/messages", self.base_url);
            
            // Separate system messages and convert messages
            let mut system_prompt = String::new();
            let mut anthropic_messages = Vec::new();
            
            for msg in messages {
                if msg.role == "system" {
                    if let Some(content) = msg.content {
                        if !system_prompt.is_empty() {
                            system_prompt.push('\n');
                        }
                        system_prompt.push_str(&content);
                    }
                } else {
                    // Convert to Anthropic message format (content as string)
                    if let Some(content) = msg.content {
                        anthropic_messages.push(serde_json::json!({
                            "role": msg.role,
                            "content": content
                        }));
                    }
                }
            }
            
            let mut request_body = serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "messages": anthropic_messages,
                "stream": false
            });
            
            if !system_prompt.is_empty() {
                request_body["system"] = serde_json::Value::String(system_prompt);
            }
            
            let response = self
                .http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .context("Failed to send request to Anthropic API")?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read error body".to_string());
                anyhow::bail!("API request failed ({}): {}", status, body);
            }

            let result: serde_json::Value = response.json().await.context("Failed to parse Anthropic JSON response")?;
            
            // Anthropic response format: {"content": [{"type": "text", "text": "..."}], ...}
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
            // OpenAI-compatible API format
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
        }
    } // chat_completion

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let is_anthropic = self.base_url.contains("anthropic.com");
        
        if is_anthropic {
            // Anthropic doesn't have a /models endpoint; return known Claude models
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
        }
    } // list_models


    pub async fn validate_key(api_key: &str, base_url: &str, model: &str) -> Result<bool> {
        let client = Client::new();
        
        // Check if this is Anthropic API
        let is_anthropic = base_url.contains("anthropic.com");
        
        if is_anthropic {
            // Anthropic API format
            let url = format!("{}/messages", base_url);
            let request_body = serde_json::json!({
                "model": model,
                "max_tokens": 8,
                "messages": [
                    {
                        "role": "user",
                        "content": "Hi"
                    }
                ],
                "stream": false
            });
            
            let response = client
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .context("Failed to connect to Anthropic API")?;
                
            Ok(response.status().is_success())
        } else {
            // OpenAI-compatible API format
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
        }
    } // validate_key
} // impl ApiClient
