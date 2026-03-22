use crate::api::types::ChatCompletionRequest;
use reqwest::header::HeaderMap;
use serde_json::Value;

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn auth_headers(&self, api_key: &str) -> HeaderMap;
    fn format_request(&self, request: &ChatCompletionRequest) -> Value;
}
