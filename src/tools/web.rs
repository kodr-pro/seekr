// tools/web.rs - Web browsing and fetching tools
//
// web_fetch: Fetches a URL and extracts text content (stripping HTML).
// web_search: Performs a DuckDuckGo search and parses results.

use anyhow::{Context, Result};
use reqwest::Client;
use scraper::{Html, Selector};

/// Fetch a web page and return its text content
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
        // Extract content matching the CSS selector
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
        // Extract all text content from the body
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

    // Truncate very long pages
    let mut result = text;
    if result.len() > 16_000 {
        result.truncate(16_000);
        result.push_str("\n... [content truncated]");
    }

    Ok(result)
}

/// Search the web using DuckDuckGo HTML and parse results
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

    // DuckDuckGo HTML results use .result class
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
}

/// Simple URL encoding for query parameters
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
}
