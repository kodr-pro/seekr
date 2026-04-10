use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::errors::ToolError;
use crate::tools::{Tool, truncate, ExecutionContext};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::json;

pub async fn web_fetch(url: &str, selector: Option<&str>) -> Result<String> {
    // SSRF Protection
    let parsed_url = url::Url::parse(url).context("Failed to parse URL")?;
    let host = parsed_url
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host"))?;
    let port = parsed_url.port_or_known_default().unwrap_or(80);

    let addrs = tokio::net::lookup_host(format!("{}:{}", host, port))
        .await
        .map_err(|e| anyhow!("DNS lookup failed for {}: {}", host, e))?;

    for addr in addrs {
        let ip = addr.ip();
        if ip.is_loopback() {
            continue; // Allow localhost for local development/testing
        }

        let is_private = match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_private()
                    || ipv4.is_link_local()
                    || ipv4.is_broadcast()
                    || ipv4.is_documentation()
                    || ipv4.is_unspecified()
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_unspecified() || (ipv6.segments()[0] & 0xfe00) == 0xfc00
            } // Unique local address
        };

        if is_private {
            return Err(ToolError::SecurityError(format!(
                "Access to private IP {} is blocked (SSRF protection)",
                ip
            ))
            .into());
        }
    }

    let client = Client::builder()
        .user_agent(format!(
            "Mozilla/5.0 (compatible; Seekr/{})",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch URL: {}", url))?;

    if !response.status().is_success() {
        return Err(
            ToolError::WebError(format!("HTTP {} fetching {}", response.status(), url)).into(),
        );
    }

    let body = response
        .text()
        .await
        .with_context(|| "Failed to read response body")?;

    let document = Html::parse_document(&body);

    let text = if let Some(sel_str) = selector {
        match Selector::parse(sel_str) {
            Ok(sel) => {
                let mut parts = Vec::new();
                for element in document.select(&sel) {
                    parts.push(element.text().collect::<Vec<_>>().join(" "));
                }
                if parts.is_empty() {
                    format!("No elements matched selector: {}", sel_str)
                } else {
                    parts.join("\n\n")
                }
            }
            Err(_) => {
                return Err(ToolError::InvalidSelector(sel_str.to_string()).into());
            }
        }
    } else {
        match Selector::parse("body") {
            Ok(body_sel) => document
                .select(&body_sel)
                .next()
                .map(|body| body.text().collect::<Vec<_>>().join(" "))
                .unwrap_or_else(|| "No body content found".to_string()),
            Err(_) => "Failed to parse body selector".to_string(),
        }
    };

    let mut result = text;
    if result.len() > 16_000 {
        result.truncate(16_000);
        result.push_str("\n... [content truncated]");
    }

    Ok(result)
} // web_fetch

pub async fn web_search(query: &str) -> Result<String> {
    let client = Client::builder()
        .user_agent(format!(
            "Mozilla/5.0 (compatible; Seekr/{})",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));

    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| "Failed to fetch DuckDuckGo results")?;

    let body = response
        .text()
        .await
        .with_context(|| "Failed to read search results")?;

    let document = Html::parse_document(&body);

    let result_sel = Selector::parse(".result")
        .or_else(|_| Selector::parse("div"))
        .map_err(|e| anyhow!("Failed to parse CSS selector: {}", e))?;
    let title_sel = Selector::parse(".result__a")
        .or_else(|_| Selector::parse("a"))
        .map_err(|e| anyhow!("Failed to parse CSS selector: {}", e))?;
    let snippet_sel = Selector::parse(".result__snippet")
        .or_else(|_| Selector::parse("span"))
        .map_err(|e| anyhow!("Failed to parse CSS selector: {}", e))?;

    let mut results = Vec::new();
    for (i, result) in document.select(&result_sel).enumerate() {
        if i >= 10 {
            break;
        }

        let title = result
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(""))
            .unwrap_or_default();

        let link = result
            .select(&title_sel)
            .next()
            .and_then(|el| el.value().attr("href"))
            .unwrap_or("")
            .to_string();

        let snippet = result
            .select(&snippet_sel)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(""))
            .unwrap_or_default();

        if !title.is_empty() {
            results.push(format!(
                "{}. {}\n   URL: {}\n   {}",
                i + 1,
                title.trim(),
                link,
                snippet.trim()
            ));
        }
    }

    if results.is_empty() {
        Ok("No search results found.".to_string())
    } else {
        Ok(results.join("\n\n"))
    }
} // web_search

fn urlencoding(input: &str) -> String {
    let mut result = String::new();
    for c in input.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                result.push(c);
            }
            ' ' => {
                result.push('+');
            }
            _ => {
                for byte in c.to_string().as_bytes() {
                    result.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    result
} // urlencoding

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Fetch a web page and return its text content.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch" },
                        "selector": { "type": "string", "description": "Optional CSS selector" }
                    },
                    "required": ["url"]
                }),
            },
        }
    } // definition
    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let url = args["url"].as_str().ok_or_else(|| anyhow!("Missing url"))?;
        let selector = args["selector"].as_str();

        let mut short_url = url.to_string();
        if short_url.len() > 40 {
            short_url.truncate(37);
            short_url.push_str("...");
        }
        let summary = format!("web_fetch {}", short_url);
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );

        let result = web_fetch(url, selector).await?;
        Ok((result, summary))
    } // execute
} // impl WebFetchTool

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    } // name
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: self.name().to_string(),
                description: "Search the web using DuckDuckGo.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "Search query" } },
                    "required": ["query"]
                }),
            },
        }
    } // definition
    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &ExecutionContext,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing query"))?;
        let summary = format!("web_search \"{}\"", truncate(query, 20));
        context.task_manager.log_activity(
            self.name(),
            &summary,
            crate::tools::task::ActivityStatus::Starting,
            thread_id,
            total_threads,
        );
        let result = web_search(query).await?;
        Ok((result, summary))
    } // execute
} // impl WebSearchTool
