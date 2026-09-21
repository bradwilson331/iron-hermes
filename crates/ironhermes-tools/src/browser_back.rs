//! Phase 25.1 D-04: browser_back — navigate back in history.
//!
//! Phase 53 Plan 01: drives navigation through the CDP `Page` history
//! commands (`Page.getNavigationHistory` / `Page.navigateToHistoryEntry`)
//! rather than evaluating a history call inside the page's own JavaScript
//! realm. chromiumoxide 0.9.1 ships the typed params for both commands, so
//! there is no longer a portability reason to route this through the page's
//! script realm — and an in-realm call is not guaranteed to be honoured by
//! every CDP engine (see ADR-0005 item 4). Waits briefly for the navigation
//! to commit, then returns the new URL. No ref table consumed, no approval
//! gating, no allowlist (history navigation only).

use std::sync::Arc;

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::page::{
    GetNavigationHistoryParams, NavigateToHistoryEntryParams, NavigationEntry,
};
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{
    BrowserSession, configured_browser_engine_available, configured_engine_prerequisite,
};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserBackTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
    config: Arc<ironhermes_core::config::Config>,
}

impl BrowserBackTool {
    pub fn new(
        session: Arc<Mutex<Option<BrowserSession>>>,
        config: Arc<ironhermes_core::config::Config>,
    ) -> Self {
        Self { session, config }
    }
}

#[async_trait]
impl Tool for BrowserBackTool {
    fn name(&self) -> &str {
        "browser_back"
    }
    fn toolset(&self) -> &str {
        "browser"
    }
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
        configured_browser_engine_available(&self.config.browser)
    }

    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![configured_engine_prerequisite(&self.config.browser)]
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        debug!("browser_back invoked");
        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        let history = sess
            .page
            .execute(GetNavigationHistoryParams::default())
            .await
            .map_err(|e| anyhow::anyhow!("Page.getNavigationHistory failed: {e}"))?;

        let target = back_target(history.current_index, &history.entries);
        let navigated_back = if let Some(entry_id) = target {
            sess.page
                .execute(NavigateToHistoryEntryParams::new(entry_id))
                .await
                .map_err(|e| anyhow::anyhow!("Page.navigateToHistoryEntry failed: {e}"))?;

            // Brief wait for the new document to commit. 250ms is empirical; if the
            // page is slow, the LLM can call browser_snapshot which has its own waits.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            true
        } else {
            false
        };

        let url = sess
            .page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "about:blank".to_string());
        Ok(json!({ "navigated_back": navigated_back, "url": url }).to_string())
    }
}

/// Decide which history entry (if any) a back navigation should target.
///
/// `current_index` and `entries` come directly from
/// `Page.getNavigationHistory`. Returns the `id` of the entry immediately
/// preceding the current one, or `None` when there is nothing to go back to
/// — the current entry is already the first one, the entry list is empty, or
/// the reported index does not resolve to a valid preceding entry.
fn back_target(current_index: i64, entries: &[NavigationEntry]) -> Option<i64> {
    if current_index <= 0 {
        return None;
    }
    let prev = usize::try_from(current_index - 1).ok()?;
    entries.get(prev).map(|entry| entry.id)
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

    fn dummy_config() -> Arc<ironhermes_core::config::Config> {
        Arc::new(ironhermes_core::config::Config::default())
    }

    #[test]
    fn name_and_toolset_match_d04() {
        let t = BrowserBackTool::new(dummy_session(), dummy_config());
        assert_eq!(t.name(), "browser_back");
        assert_eq!(t.toolset(), "browser");
    }

    #[test]
    fn schema_has_no_required_args() {
        let t = BrowserBackTool::new(dummy_session(), dummy_config());
        let s = t.schema();
        let params = s.function.parameters;
        let required = params.get("required").and_then(|v| v.as_array());
        assert!(
            required.map(|a| a.is_empty()).unwrap_or(true),
            "browser_back takes no required args"
        );
    }

    /// Phase 53 Plan 04: the configured engine's binary_present prerequisite —
    /// the default config's backend is Chromium, so this still names
    /// chromium-or-chrome (Task 2 delegates to configured_engine_prerequisite
    /// rather than a hand-rolled literal).
    #[test]
    fn prerequisites_declare_chromium_binary() {
        let t = BrowserBackTool::new(dummy_session(), dummy_config());
        let p = t.prerequisites();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, "binary_present");
        assert_eq!(p[0].name, "chromium-or-chrome");
        assert!(p[0].required);
    }

    fn nav_entry(id: i64, url: &str) -> NavigationEntry {
        NavigationEntry {
            id,
            url: url.to_string(),
            user_typed_url: url.to_string(),
            title: String::new(),
            transition_type: chromiumoxide::cdp::browser_protocol::page::TransitionType::Link,
        }
    }

    #[test]
    fn browser_back_issues_cdp_history_commands_not_in_realm_javascript() {
        let src = include_str!("browser_back.rs");
        // The old mechanism drove navigation by evaluating script inside the
        // page's own realm. Build the needle at runtime (rather than as one
        // contiguous literal) so this self-referential source scan finds a
        // real call in production code, not the needle string in its own
        // assertion below.
        let in_realm_call = format!("{}{}", "page.eval", "uate(");
        assert!(
            !src.contains(&in_realm_call),
            "browser_back.rs must not evaluate JavaScript in the page realm for navigation"
        );
        assert!(
            src.contains("GetNavigationHistoryParams"),
            "browser_back.rs must issue Page.getNavigationHistory"
        );
        assert!(
            src.contains("NavigateToHistoryEntryParams"),
            "browser_back.rs must issue Page.navigateToHistoryEntry"
        );
    }

    #[test]
    fn browser_back_at_the_first_history_entry_reports_no_navigation() {
        let single = vec![nav_entry(1, "https://a.example/")];
        // Already at the oldest entry — nothing precedes it.
        assert_eq!(back_target(0, &single), None);
        // Defensive: an index that does not resolve into the entry list must
        // not be treated as a navigable target either.
        assert_eq!(back_target(5, &single), None);
        assert_eq!(back_target(1, &[]), None);

        let two = vec![
            nav_entry(10, "https://a.example/"),
            nav_entry(20, "https://b.example/"),
        ];
        assert_eq!(back_target(1, &two), Some(10));
    }
}
