//! Phase 25.1 D-04: browser_scroll — scroll the page up/down by page/half/pixels.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserScrollTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserScrollTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserScrollTool {
    fn name(&self) -> &str { "browser_scroll" }
    fn toolset(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Scroll the page. direction: 'up' | 'down' (default 'down'). amount: 'page' | 'half' | <integer pixels> (default 'page')."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_scroll",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down"],
                        "description": "Direction to scroll.",
                        "default": "down"
                    },
                    "amount": {
                        "description": "How much to scroll. 'page' = one viewport, 'half' = half viewport, integer = literal pixels.",
                        "default": "page"
                    }
                },
                "required": []
            }),
        )
    }

    fn is_available(&self) -> bool { find_chromium_binary(None).is_some() }

    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![Prerequisite {
            kind: "binary_present".to_string(),
            name: "chromium-or-chrome".to_string(),
            description: "Chromium or Google Chrome browser binary on PATH or at a standard install location"
                .to_string(),
            required: true,
        }]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
        if !matches!(direction, "up" | "down") {
            return Err(anyhow::anyhow!(
                "Invalid direction '{}'. Allowed: 'up' | 'down'", direction
            ));
        }
        let sign: i64 = if direction == "down" { 1 } else { -1 };

        // amount: "page" | "half" | integer literal
        let amount_js: String = match args.get("amount") {
            Some(v) if v.as_str() == Some("page") || v.is_null() => {
                format!("({}) * window.innerHeight", sign)
            }
            None => format!("({}) * window.innerHeight", sign),
            Some(v) if v.as_str() == Some("half") => {
                format!("({}) * (window.innerHeight / 2)", sign)
            }
            Some(v) if v.is_number() => {
                let pixels = v.as_i64().unwrap_or(0);
                format!("{}", sign * pixels)
            }
            Some(v) if v.as_str().is_some() => {
                return Err(anyhow::anyhow!(
                    "Invalid amount '{}'. Allowed: 'page' | 'half' | <integer pixels>",
                    v.as_str().unwrap_or("")
                ));
            }
            _ => format!("({}) * window.innerHeight", sign),
        };

        debug!(direction, amount_js, "browser_scroll");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        let js = format!(
            r#"(function() {{
                const dy = {amount_js};
                window.scrollBy(0, dy);
                return {{ scrolled_y: dy, scroll_y: window.scrollY, max: document.body.scrollHeight }};
            }})()"#
        );

        let result = sess
            .page
            .evaluate(js.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("scroll failed: {e}"))?;

        let value: serde_json::Value = result.into_value().unwrap_or(serde_json::Value::Null);

        Ok(json!({
            "scrolled": direction,
            "result": value
        }).to_string())
    }
}

async fn ensure_session<'a>(
    guard: &'a mut tokio::sync::MutexGuard<'_, Option<BrowserSession>>,
) -> anyhow::Result<&'a mut BrowserSession> {
    if guard.is_none() {
        let cfg = ironhermes_core::config::Config::load().unwrap_or_default().browser;
        let new = BrowserSession::spawn(&cfg).await?;
        **guard = Some(new);
    }
    Ok(guard.as_mut().expect("just inserted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_session() -> Arc<Mutex<Option<BrowserSession>>> {
        Arc::new(Mutex::new(None))
    }

    #[test]
    fn name_and_toolset_match_d04() {
        let t = BrowserScrollTool::new(dummy_session());
        assert_eq!(t.name(), "browser_scroll");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_direction() {
        let t = BrowserScrollTool::new(dummy_session());
        let result = t.execute(json!({"direction": "left"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid direction"));
    }

    #[tokio::test]
    async fn execute_rejects_unknown_amount_string() {
        let t = BrowserScrollTool::new(dummy_session());
        let result = t.execute(json!({"amount": "huge"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid amount"));
    }
}
