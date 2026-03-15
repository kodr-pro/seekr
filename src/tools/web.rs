use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use scraper::{Html, Selector};
use async_trait::async_trait;
use crate::api::types::{FunctionDefinition, ToolDefinition};
use crate::tools::{Tool, task::TaskManager, truncate};
use serde_json::json;

pub async fn web_fetch(url: &str, selector: Option<&str>) -> Result<String> {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (compatible; Seekr/0.1)")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch URL: {}", url))?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", response.status(), url);
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
                anyhow::bail!("Invalid CSS selector: {}", sel_str);
            }
        }
    } else {
        match Selector::parse("body") {
            Ok(body_sel) => {
                document
                    .select(&body_sel)
                    .next()
                    .map(|body| body.text().collect::<Vec<_>>().join(" "))
                    .unwrap_or_else(|| "No body content found".to_string())
            }
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
        .user_agent("Mozilla/5.0 (compatible; Seekr/0.1)")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding(query)
    );

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

    let result_sel = Selector::parse(".result").unwrap_or_else(|_| {
        Selector::parse("div").expect("div selector should always work")
    });
    let title_sel = Selector::parse(".result__a").unwrap_or_else(|_| {
        Selector::parse("a").expect("a selector should always work")
    });
    let snippet_sel = Selector::parse(".result__snippet").unwrap_or_else(|_| {
        Selector::parse("span").expect("span selector should always work")
    });

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
    fn name(&self) -> &str { "web_fetch" } // name
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
        task_manager: &TaskManager,
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
        task_manager.log_activity(self.name(), &summary, crate::tools::task::ActivityStatus::Starting, thread_id, total_threads);
        
        let result = web_fetch(url, selector).await?;
        Ok((result, summary))
    } // execute
} // impl WebFetchTool

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" } // name
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
        task_manager: &TaskManager,
        thread_id: Option<usize>,
        total_threads: Option<usize>,
    ) -> Result<(String, String)> {
        let query = args["query"].as_str().ok_or_else(|| anyhow!("Missing query"))?;
        let summary = format!("web_search \"{}\"", truncate(query, 20));
        task_manager.log_activity(self.name(), &summary, crate::tools::task::ActivityStatus::Starting, thread_id, total_threads);
        let result = web_search(query).await?;
        Ok((result, summary))
    } // execute
} // impl WebSearchTool
