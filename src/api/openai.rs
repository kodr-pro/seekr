use crate::api::provider::Provider;
use crate::api::types::ChatCompletionRequest;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

pub struct OpenAiProvider;

impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "OpenAI Compatible"
    }

    fn auth_headers(&self, api_key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
            headers.insert(AUTHORIZATION, val);
        }
        headers
    }

    fn format_request(&self, request: &ChatCompletionRequest) -> Value {
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "stream": request.stream,
        });

        if let Some(tokens) = request.max_tokens {
            body["max_tokens"] = json!(tokens);
        }

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                // OpenAI format: { "type": "function", "function": { ... } }
                body["tools"] = json!(tools);
                body["tool_choice"] = json!("auto");
            }
        }

        body
    }
}
