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

use crate::browser_session::{BrowserSession, find_chromium_binary};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserNavigateTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
    config: Arc<ironhermes_core::config::Config>,
}

impl BrowserNavigateTool {
    pub fn new(
        session: Arc<Mutex<Option<BrowserSession>>>,
        config: Arc<ironhermes_core::config::Config>,
    ) -> Self {
        Self { session, config }
    }
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str {
        "browser_navigate"
    }
    fn toolset(&self) -> &str {
        "browser"
    }
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

    fn is_available(&self) -> bool {
        find_chromium_binary(None).is_some()
    }

    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![Prerequisite {
            kind: "binary_present".to_string(),
            name: "chromium-or-chrome".to_string(),
            description:
                "Chromium or Google Chrome browser binary on PATH or at a standard install location"
                    .to_string(),
            required: true,
        }]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?
            .to_string();

        let cfg = &self.config.browser;

        // D-16: scheme allowlist. Default ["http","https"]. Reject everything else (file:, data:, javascript:).
        let scheme = url
            .split_once("://")
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| {
                // Bare schemes like "javascript:..." or "data:..." (no //)
                url.split_once(':')
                    .map(|(s, _)| s.to_string())
                    .unwrap_or_default()
            });
        if !cfg.allowed_schemes.iter().any(|s| s == &scheme) {
            return Ok(json!({
                "error": "scheme_blocked",
                "url": url,
                "scheme": scheme,
                "allowed": cfg.allowed_schemes.clone(),
                "hint": "Add the scheme to browser.allowed_schemes to permit"
            })
            .to_string());
        }

        // D-15: host allowlist (empty = allow all).
        if let Err(e) = BrowserSession::validate_navigation_url(&cfg.allowed_domains, &url) {
            // The validator already produces a structured error envelope as the err message.
            // Return it as Ok-string so the LLM sees the structured rejection.
            return Ok(e.to_string());
        }

        debug!(%url, "browser_navigate");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard, &self.config.browser).await?;

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
        })
        .to_string())
    }
}

async fn ensure_session<'a>(
    guard: &'a mut tokio::sync::MutexGuard<'_, Option<BrowserSession>>,
    browser_cfg: &ironhermes_core::config::BrowserConfig,
) -> anyhow::Result<&'a mut BrowserSession> {
    if guard.is_none() {
        let new = BrowserSession::spawn(browser_cfg).await?;
        **guard = Some(new);
    }
    Ok(guard.as_mut().expect("just inserted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::{BrowserConfig, Config};

    fn dummy_session() -> Arc<Mutex<Option<BrowserSession>>> {
        Arc::new(Mutex::new(None))
    }

    fn dummy_navigate_tool(allowed_domains: Vec<String>) -> BrowserNavigateTool {
        let mut config = Config::default();
        config.browser = BrowserConfig {
            allowed_domains,
            ..BrowserConfig::default()
        };
        BrowserNavigateTool::new(dummy_session(), Arc::new(config))
    }

    #[test]
    fn name_and_toolset_match_d04() {
        let t = dummy_navigate_tool(vec![]);
        assert_eq!(t.name(), "browser_navigate");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn execute_rejects_missing_url() {
        let t = dummy_navigate_tool(vec![]);
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing required parameter: url")
        );
    }

    #[tokio::test]
    async fn execute_rejects_disallowed_scheme() {
        // Default config has allowed_schemes = ["http", "https"]
        let t = dummy_navigate_tool(vec![]);
        let result = t
            .execute(json!({"url": "javascript:alert(1)"}))
            .await
            .unwrap();
        assert!(result.contains("\"error\":\"scheme_blocked\""));
        assert!(result.contains("javascript"));
    }

    #[tokio::test]
    async fn execute_rejects_file_scheme_by_default() {
        let t = dummy_navigate_tool(vec![]);
        let result = t
            .execute(json!({"url": "file:///etc/passwd"}))
            .await
            .unwrap();
        assert!(result.contains("\"error\":\"scheme_blocked\""));
    }

    /// GAP-3 / T-25.1-01: allowlist enforcement uses the injected Config, not BrowserConfig::default().
    /// Proves that when allowed_domains=["example.com"], navigation to wikipedia.org is blocked.
    #[tokio::test]
    async fn execute_blocks_navigate_when_host_not_in_allowlist() {
        let t = dummy_navigate_tool(vec!["example.com".to_string()]);
        let result = t
            .execute(json!({"url": "https://wikipedia.org"}))
            .await
            .unwrap();
        assert!(
            result.contains("\"error\":\"domain_blocked\""),
            "expected domain_blocked, got: {result}"
        );
        assert!(
            result.contains("wikipedia.org"),
            "host must appear in envelope: {result}"
        );
        assert!(
            result.contains("example.com"),
            "allowed list must appear in envelope: {result}"
        );
    }

    /// GAP-3 / T-25.1-01: uses a DISTINCT_TEST_HOSTNAME that no real config file would ever contain.
    /// If the tool were calling Config::load() from disk, the rejection envelope would contain
    /// whatever the test machine's config file has (typically empty list = no rejection at all).
    /// The presence of DISTINCT_TEST_HOSTNAME.example proves the injected Config is the source.
    #[tokio::test]
    async fn execute_uses_injected_config_not_disk() {
        let t = dummy_navigate_tool(vec!["DISTINCT_TEST_HOSTNAME.example".to_string()]);
        let result = t
            .execute(json!({"url": "https://wikipedia.org"}))
            .await
            .unwrap();
        // Must be rejected (domain_blocked) — not a scheme_blocked or success
        assert!(
            result.contains("\"error\":\"domain_blocked\""),
            "expected domain_blocked, got: {result}"
        );
        // The injected distinct hostname must appear in the allowed list in the envelope
        assert!(
            result.contains("DISTINCT_TEST_HOSTNAME.example"),
            "injected allowed_domains must appear in rejection envelope — proves disk Config::load() is NOT used: {result}"
        );
    }
}
