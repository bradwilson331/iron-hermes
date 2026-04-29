use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::registry::Tool;

pub struct WebSearchTool;

#[derive(Debug, Deserialize)]
struct FirecrawlSearchResponse {
    success: bool,
    #[serde(default)]
    data: Vec<FirecrawlSearchResult>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FirecrawlSearchResult {
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Search the web and return relevant results (title, URL, snippet)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "web_search",
            "Search the web and return relevant results (title, URL, snippet).",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 10).",
                        "default": 10
                    }
                },
                "required": ["query"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        std::env::var("FIRECRAWL_API_KEY").is_ok()
    }

    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![crate::registry::Prerequisite {
            kind: "env_var".to_string(),
            name: "FIRECRAWL_API_KEY".to_string(),
            description: "Firecrawl API key — required for web_search to query the live web.".to_string(),
            required: true,
        }]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;
        let limit = args["limit"].as_u64().unwrap_or(10);

        let api_key = std::env::var("FIRECRAWL_API_KEY")
            .map_err(|_| anyhow::anyhow!("FIRECRAWL_API_KEY environment variable not set"))?;

        debug!("Searching web for: {}", query);

        let client = reqwest::Client::new();
        let response = client
            .post("https://api.firecrawl.dev/v1/search")
            .bearer_auth(&api_key)
            .json(&json!({
                "query": query,
                "limit": limit
            }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Firecrawl API returned {}: {}",
                status,
                body
            ));
        }

        let search_response: FirecrawlSearchResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        if !search_response.success {
            return Err(anyhow::anyhow!(
                "Firecrawl search failed: {}",
                search_response.error.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        if search_response.data.is_empty() {
            return Ok(format!("No results found for '{}'.", query));
        }

        let mut output = format!(
            "{} result(s) for '{}':\n\n",
            search_response.data.len(),
            query
        );

        for (i, result) in search_response.data.iter().enumerate() {
            let title = result.title.as_deref().unwrap_or("(no title)");
            let snippet = result
                .description
                .as_deref()
                .or(result.markdown.as_deref())
                .unwrap_or("")
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(300)
                .collect::<String>();

            output.push_str(&format!(
                "{}. {}\n   URL: {}\n   {}\n\n",
                i + 1,
                title,
                result.url,
                snippet
            ));
        }

        Ok(output.trim_end().to_string())
    }
}
