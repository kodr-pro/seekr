use crate::api::provider::Provider;
use crate::api::types::ChatCompletionRequest;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

pub struct AnthropicProvider;

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "Anthropic"
    }

    fn auth_headers(&self, api_key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("messages-2023-12-15"),
        );
        if let Ok(val) = HeaderValue::from_str(api_key) {
            headers.insert("x-api-key", val);
        }
        headers
    }

    fn format_request(&self, request: &ChatCompletionRequest) -> Value {
        // Separate system messages
        let mut system_prompt = String::new();
        let mut anthropic_messages = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                if !system_prompt.is_empty() {
                    system_prompt.push_str("\n\n");
                }
                if let Some(content) = &msg.content {
                    system_prompt.push_str(content);
                }
            } else if msg.role == "assistant"
                && msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
            {
                // Formatting assistant tool calls for Anthropic
                let mut content_blocks = Vec::new();
                if let Some(content) = &msg.content
                    && !content.is_empty()
                {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": content
                    }));
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        let input_val: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input_val
                        }));
                    }
                }
                anthropic_messages.push(json!({
                    "role": "assistant",
                    "content": content_blocks
                }));
            } else if msg.role == "tool" {
                // Formatting tool results for Anthropic
                let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                let tool_result_block = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": msg.content
                });

                let mut merge_with_previous = false;
                if let Some(last_msg) = anthropic_messages.last_mut()
                    && last_msg["role"] == "user"
                    && let Some(content_array) = last_msg["content"].as_array_mut()
                {
                    content_array.push(tool_result_block.clone());
                    merge_with_previous = true;
                }

                if !merge_with_previous {
                    anthropic_messages.push(json!({
                        "role": "user",
                        "content": [tool_result_block]
                    }));
                }
            } else {
                anthropic_messages.push(json!({
                    "role": msg.role,
                    "content": msg.content
                }));
            }
        }

        let mut body = json!({
            "model": request.model,
            "messages": anthropic_messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "stream": request.stream,
        });

        if !system_prompt.is_empty() {
            body["system"] = json!(system_prompt);
        }

        if let Some(tools) = &request.tools
            && !tools.is_empty()
        {
            let mut anthropic_tools = Vec::new();
            for t in tools {
                anthropic_tools.push(json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters
                }));
            }
            body["tools"] = json!(anthropic_tools);
        }

        body
    }
}
