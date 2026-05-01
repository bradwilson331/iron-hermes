//! Phase 25.1 D-04: browser_back — navigate back in history.
//!
//! Calls `window.history.back()` via page.evaluate. Waits briefly for the
//! navigation to commit, then returns the new URL. No ref table consumed,
//! no approval gating, no allowlist (history navigation only).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserBackTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserBackTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserBackTool {
    fn name(&self) -> &str { "browser_back" }
    fn toolset(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Navigate back in browser history. Returns the new URL after the back navigation."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_back",
            self.description(),
            json!({
                "type": "object",
                "properties": {},
                "required": []
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
            description: "Chromium or Google Chrome browser binary on PATH or at a standard install location"
                .to_string(),
            required: true,
        }]
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        debug!("browser_back invoked");
        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // Page.goBack via JS — CDP-version-agnostic per RESEARCH §"Navigation API"
        let _ = sess.page.evaluate("window.history.back()").await
            .map_err(|e| anyhow::anyhow!("history.back failed: {e}"))?;

        // Brief wait for the new document to commit. 250ms is empirical; if the
        // page is slow, the LLM can call browser_snapshot which has its own waits.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let url = sess.page.url().await
            .ok()
            .flatten()
            .unwrap_or_else(|| "about:blank".to_string());
        Ok(json!({ "navigated_back": true, "url": url }).to_string())
    }
}

/// Shared lazy-spawn helper used by every browser_* tool's execute().
/// Reads BrowserConfig from disk, spawns a session if None, returns &mut to it.
async fn ensure_session<'a>(
    guard: &'a mut tokio::sync::MutexGuard<'_, Option<BrowserSession>>,
) -> anyhow::Result<&'a mut BrowserSession> {
    if guard.is_none() {
        let cfg = ironhermes_core::config::Config::load()
            .unwrap_or_default()
            .browser;
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
        let t = BrowserBackTool::new(dummy_session());
        assert_eq!(t.name(), "browser_back");
        assert_eq!(t.toolset(), "browser");
    }

    #[test]
    fn schema_has_no_required_args() {
        let t = BrowserBackTool::new(dummy_session());
        let s = t.schema();
        let params = s.function.parameters;
        let required = params.get("required").and_then(|v| v.as_array());
        assert!(required.map(|a| a.is_empty()).unwrap_or(true),
            "browser_back takes no required args");
    }

    #[test]
    fn prerequisites_declare_chromium_binary() {
        let t = BrowserBackTool::new(dummy_session());
        let p = t.prerequisites();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, "binary_present");
        assert_eq!(p[0].name, "chromium-or-chrome");
        assert!(p[0].required);
    }
}
