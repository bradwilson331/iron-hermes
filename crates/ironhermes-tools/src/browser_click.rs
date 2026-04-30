//! Phase 25.1 D-04 / D-10 / D-11: browser_click — click an element by snapshot ref.
//!
//! Threat anchor T-25.1-03 (stale-ref click): on cache miss OR chromiumoxide
//! find_element error, returns the D-11 element_stale envelope so the LLM knows
//! to re-snapshot. NO automatic re-snapshot.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserClickTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserClickTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

fn element_stale(ref_id: u64) -> String {
    json!({
        "error": "element_stale",
        "ref": ref_id,
        "hint": format!("ref {ref_id} not found in current snapshot — call browser_snapshot first")
    }).to_string()
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str { "browser_click" }
    fn toolset(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Click an element by its ref ID (refs come from browser_snapshot or browser_get_images). \
         If the ref no longer exists (DOM mutated, or page navigated), returns \
         {error: 'element_stale', ref: N, hint: '...call browser_snapshot first'}."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_click",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Element ref ID from browser_snapshot or browser_get_images."
                    },
                    "modifiers": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["ctrl", "shift", "alt", "meta"] },
                        "description": "Optional click modifiers (e.g. ctrl-click for new tab).",
                        "default": []
                    }
                },
                "required": ["ref"]
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
        let ref_id = args
            .get("ref")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: ref (integer)"))?;

        let modifiers: Vec<String> = args
            .get("modifiers")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();

        debug!(ref_id, ?modifiers, "browser_click");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // D-11: cache miss → structured envelope (Ok, not Err — the LLM consumes the JSON).
        let selector = match sess.ref_table.get(&ref_id) {
            Some(s) => s.clone(),
            None => return Ok(element_stale(ref_id)),
        };

        if modifiers.is_empty() {
            // Standard chromiumoxide click path.
            match sess.page.find_element(&selector).await {
                Ok(el) => {
                    if let Err(e) = el.click().await {
                        debug!(error = %e, "click failed — likely DOM mutated");
                        return Ok(element_stale(ref_id));
                    }
                }
                Err(e) => {
                    debug!(error = %e, "find_element failed — element gone");
                    return Ok(element_stale(ref_id));
                }
            }
        } else {
            // Modifier click via JS dispatchEvent.
            let ctrl = modifiers.iter().any(|m| m == "ctrl");
            let shift = modifiers.iter().any(|m| m == "shift");
            let alt = modifiers.iter().any(|m| m == "alt");
            let meta = modifiers.iter().any(|m| m == "meta");
            let js = format!(
                r#"(function() {{
                    const el = document.querySelector({selector_lit});
                    if (!el) return false;
                    const evt = new MouseEvent('click', {{
                        bubbles: true, cancelable: true,
                        ctrlKey: {ctrl}, shiftKey: {shift}, altKey: {alt}, metaKey: {meta}
                    }});
                    el.dispatchEvent(evt);
                    return true;
                }})()"#,
                selector_lit = serde_json::to_string(&selector).unwrap_or("\"\"".to_string())
            );
            let result = sess.page.evaluate(js.as_str()).await
                .map_err(|e| anyhow::anyhow!("modifier click failed: {e}"))?;
            let succeeded = result.into_value::<bool>().unwrap_or(false);
            if !succeeded {
                return Ok(element_stale(ref_id));
            }
        }

        Ok(json!({ "clicked": true, "ref": ref_id, "modifiers": modifiers }).to_string())
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
        let t = BrowserClickTool::new(dummy_session());
        assert_eq!(t.name(), "browser_click");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn execute_rejects_missing_ref() {
        let t = BrowserClickTool::new(dummy_session());
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing required parameter: ref"));
    }

    #[test]
    fn element_stale_envelope_shape_matches_d11() {
        let s = element_stale(7);
        assert!(s.contains("\"error\":\"element_stale\""));
        assert!(s.contains("\"ref\":7"));
        assert!(s.contains("call browser_snapshot first"));
    }
}
