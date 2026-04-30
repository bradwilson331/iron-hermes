//! Phase 25.1 D-04 / D-10 / OQ-1: browser_snapshot.
//!
//! Walks the DOM via page.evaluate, identifies interactive elements, assigns
//! sequential integer refs (1..N), AND writes `data-ironhermes-ref="N"`
//! attributes on each so plan 06 (browser_click/browser_type) can target
//! by `document.querySelector('[data-ironhermes-ref="N"]')`. This avoids
//! the CDP BackendNodeId-click uncertainty (RESEARCH OQ-1 fix).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

/// JS injected into the page to walk the DOM, assign refs, and decorate elements.
const SNAPSHOT_WALKER_JS: &str = r#"
(function() {
    // First, clear any data-ironhermes-ref from a previous snapshot.
    for (const el of document.querySelectorAll('[data-ironhermes-ref]')) {
        el.removeAttribute('data-ironhermes-ref');
    }

    const INTERACTIVE_TAGS = new Set(['A', 'BUTTON', 'INPUT', 'TEXTAREA', 'SELECT']);
    const INTERACTIVE_ROLES = new Set([
        'button', 'link', 'textbox', 'checkbox', 'radio', 'combobox',
        'menuitem', 'tab', 'searchbox', 'spinbutton', 'slider'
    ]);
    const STRUCTURAL_TAGS = new Set([
        'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'NAV', 'MAIN', 'SECTION', 'ARTICLE',
        'FOOTER', 'HEADER', 'ASIDE'
    ]);

    function roleOf(el) {
        const explicit = el.getAttribute('role');
        if (explicit) return explicit;
        const tag = el.tagName;
        if (tag === 'A' && el.hasAttribute('href')) return 'link';
        if (tag === 'BUTTON') return 'button';
        if (tag === 'TEXTAREA') return 'textbox';
        if (tag === 'SELECT') return 'combobox';
        if (tag === 'INPUT') {
            const t = (el.getAttribute('type') || 'text').toLowerCase();
            if (t === 'button' || t === 'submit' || t === 'reset') return 'button';
            if (t === 'checkbox') return 'checkbox';
            if (t === 'radio') return 'radio';
            if (t === 'search') return 'searchbox';
            if (t === 'number') return 'spinbutton';
            if (t === 'range') return 'slider';
            return 'textbox';
        }
        if (tag === 'H1' || tag === 'H2' || tag === 'H3' || tag === 'H4' || tag === 'H5' || tag === 'H6') return 'heading';
        if (tag === 'NAV') return 'navigation';
        if (tag === 'MAIN') return 'main';
        if (tag === 'FOOTER') return 'contentinfo';
        if (tag === 'HEADER') return 'banner';
        if (tag === 'ASIDE') return 'complementary';
        if (tag === 'ARTICLE') return 'article';
        if (tag === 'SECTION') return 'region';
        return null;
    }

    function nameOf(el) {
        const aria = el.getAttribute('aria-label');
        if (aria) return aria.trim();
        const labelledby = el.getAttribute('aria-labelledby');
        if (labelledby) {
            const ref = document.getElementById(labelledby);
            if (ref) return (ref.textContent || '').trim();
        }
        if (el.tagName === 'INPUT') {
            const ph = el.getAttribute('placeholder');
            if (ph) return ph.trim();
            // Try associated <label for=id>
            if (el.id) {
                const label = document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
                if (label) return (label.textContent || '').trim();
            }
            const name = el.getAttribute('name');
            if (name) return name.trim();
        }
        const text = (el.textContent || '').trim();
        // Limit to 80 chars for snapshot compactness
        return text.length > 80 ? text.slice(0, 77) + '...' : text;
    }

    function isInteractive(el) {
        if (INTERACTIVE_TAGS.has(el.tagName)) return true;
        const r = el.getAttribute('role');
        if (r && INTERACTIVE_ROLES.has(r)) return true;
        return false;
    }
    function isStructural(el) {
        return STRUCTURAL_TAGS.has(el.tagName);
    }

    let counter = 0;
    const out = [];

    function walk(el, depth) {
        if (!el || el.nodeType !== 1) return;
        // Skip non-visible elements (display:none / hidden) for compactness.
        const style = window.getComputedStyle(el);
        if (style && (style.display === 'none' || style.visibility === 'hidden')) return;

        const interactive = isInteractive(el);
        const structural = isStructural(el);
        const role = roleOf(el);

        if (interactive) {
            counter += 1;
            el.setAttribute('data-ironhermes-ref', String(counter));
            const name = nameOf(el);
            out.push({ ref: counter, role: role || el.tagName.toLowerCase(), name, depth, interactive: true });
        } else if (structural && role) {
            const name = nameOf(el);
            out.push({ ref: 0, role, name, depth, interactive: false });
        }

        const nextDepth = (interactive || structural) ? depth + 1 : depth;
        for (const child of el.children) {
            walk(child, nextDepth);
        }
    }

    walk(document.body, 0);
    return out;
})()
"#;

pub struct BrowserSnapshotTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserSnapshotTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn name(&self) -> &str {
        "browser_snapshot"
    }

    fn toolset(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Capture a text-based accessibility-tree snapshot of the current page. \
         Each interactive element gets a sequential integer ref like `[3] button \"Submit\"`. \
         Pass these refs to browser_click / browser_type. Refs are invalidated by the next \
         browser_snapshot call or any browser_navigate."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_snapshot",
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
        debug!("browser_snapshot invoked");
        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // D-10: ref_table is INVALIDATED on every snapshot — clear before populating.
        sess.ref_table.clear();

        let result = sess
            .page
            .evaluate(SNAPSHOT_WALKER_JS)
            .await
            .map_err(|e| anyhow::anyhow!("snapshot walker failed: {e}"))?;

        let entries: Vec<serde_json::Value> = result
            .into_value()
            .unwrap_or_else(|_| serde_json::Value::Array(vec![]))
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut output = String::new();
        for entry_val in &entries {
            // Hand-decode rather than serde — both `ref` (u64) and `ref:0` (structural marker) come through.
            let r = entry_val.get("ref").and_then(|v| v.as_u64()).unwrap_or(0);
            let role = entry_val.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let name = entry_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let depth = entry_val.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let interactive = entry_val
                .get("interactive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let indent = "  ".repeat(depth.min(20));

            if interactive && r > 0 {
                // Register selector in ref_table so plan 06 click/type can find this element.
                sess.ref_table.insert(r, format!("[data-ironhermes-ref=\"{r}\"]"));
                if name.is_empty() {
                    output.push_str(&format!("{indent}[{r}] {role}\n"));
                } else {
                    output.push_str(&format!("{indent}[{r}] {role} \"{name}\"\n"));
                }
            } else if !role.is_empty() {
                if name.is_empty() {
                    output.push_str(&format!("{indent}{role}\n"));
                } else {
                    output.push_str(&format!("{indent}{role} \"{name}\"\n"));
                }
            }
        }

        if output.is_empty() {
            output.push_str("(empty page or no interactive elements)\n");
        }
        Ok(output)
    }
}

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
        let t = BrowserSnapshotTool::new(dummy_session());
        assert_eq!(t.name(), "browser_snapshot");
        assert_eq!(t.toolset(), "browser");
    }

    #[test]
    fn snapshot_walker_js_clears_old_refs_first() {
        // Static-text invariant: walker JS MUST contain the cleanup loop so a re-snapshot
        // doesn't inherit stale refs from a previous snapshot.
        assert!(SNAPSHOT_WALKER_JS.contains("removeAttribute('data-ironhermes-ref')"));
    }

    #[test]
    fn snapshot_walker_js_assigns_data_attribute() {
        // OQ-1 fix invariant: walker MUST set data-ironhermes-ref on each interactive element.
        assert!(SNAPSHOT_WALKER_JS.contains("setAttribute('data-ironhermes-ref'"));
    }
}
