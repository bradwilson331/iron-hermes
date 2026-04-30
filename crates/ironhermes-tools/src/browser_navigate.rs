//! Phase 25.1 D-04: browser_navigate — load a URL into the browser session.
//!
//! Threat model anchors:
//!   - T-25.1-01 (SSRF): D-15 host allowlist; D-16 scheme allowlist
//!   - T-25.1-08 (console PII leak): clears console_buffer on every navigate

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserNavigateTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserNavigateTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str { "browser_navigate" }
    fn toolset(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Navigate the browser session to a URL. Validated against browser.allowed_domains \
         and browser.allowed_schemes. Returns {status, url, title} on success or \
         {error: 'domain_blocked'|'scheme_blocked', ...} envelope on rejection."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_navigate",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to navigate to (validated against allowlist)."
                    },
                    "wait_until": {
                        "type": "string",
                        "enum": ["domcontentloaded", "load", "networkidle"],
                        "description": "Navigation wait condition.",
                        "default": "domcontentloaded"
                    }
                },
                "required": ["url"]
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
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?
            .to_string();

        let cfg = ironhermes_core::config::Config::load()
            .unwrap_or_default()
            .browser;

        // D-16: scheme allowlist. Default ["http","https"]. Reject everything else (file:, data:, javascript:).
        let scheme = url
            .split_once("://")
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| {
                // Bare schemes like "javascript:..." or "data:..." (no //)
                url.split_once(':').map(|(s, _)| s.to_string()).unwrap_or_default()
            });
        if !cfg.allowed_schemes.iter().any(|s| s == &scheme) {
            return Ok(json!({
                "error": "scheme_blocked",
                "url": url,
                "scheme": scheme,
                "allowed": cfg.allowed_schemes,
                "hint": "Add the scheme to browser.allowed_schemes to permit"
            }).to_string());
        }

        // D-15: host allowlist (empty = allow all).
        if let Err(e) = BrowserSession::validate_navigation_url(&cfg.allowed_domains, &url) {
            // The validator already produces a structured error envelope as the err message.
            // Return it as Ok-string so the LLM sees the structured rejection.
            return Ok(e.to_string());
        }

        debug!(%url, "browser_navigate");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // T-25.1-08: clear console buffer on every navigate (avoids cross-page PII leak).
        sess.console_buffer.clear();
        // D-10: ref_table is INVALIDATED on navigation. Clear so subsequent click/type
        // against an old ref returns a clean element_stale error instead of a wrong-element click.
        sess.ref_table.clear();

        sess.page
            .goto(&url)
            .await
            .map_err(|e| anyhow::anyhow!("navigate failed: {e}"))?;
        let _ = sess.page.wait_for_navigation().await;

        let final_url = sess
            .page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.clone());

        let title = sess
            .page
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|r| r.into_value::<String>().ok())
            .unwrap_or_default();

        Ok(json!({
            "status": 200,                  // chromiumoxide goto returns Err on non-2xx; success path = 200
            "url": final_url,
            "title": title
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
        let t = BrowserNavigateTool::new(dummy_session());
        assert_eq!(t.name(), "browser_navigate");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn execute_rejects_missing_url() {
        let t = BrowserNavigateTool::new(dummy_session());
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing required parameter: url"));
    }

    #[tokio::test]
    async fn execute_rejects_disallowed_scheme() {
        // Default config has allowed_schemes = ["http", "https"]
        let t = BrowserNavigateTool::new(dummy_session());
        let result = t.execute(json!({"url": "javascript:alert(1)"})).await.unwrap();
        assert!(result.contains("\"error\":\"scheme_blocked\""));
        assert!(result.contains("javascript"));
    }

    #[tokio::test]
    async fn execute_rejects_file_scheme_by_default() {
        let t = BrowserNavigateTool::new(dummy_session());
        let result = t.execute(json!({"url": "file:///etc/passwd"})).await.unwrap();
        assert!(result.contains("\"error\":\"scheme_blocked\""));
    }
}
