//! Phase 25.1 D-04 / D-10 / D-11: browser_type — type into an input by snapshot ref.
//!
//! Mirrors browser_click's ref-resolution + stale-envelope pattern. Optional
//! `submit: true` arg presses Enter after typing (common form-submit shortcut).

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{BrowserSession, find_chromium_binary};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserTypeTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserTypeTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

fn element_stale(ref_id: u64) -> String {
    json!({
        "error": "element_stale",
        "ref": ref_id,
        "hint": format!("ref {ref_id} not found in current snapshot — call browser_snapshot first")
    })
    .to_string()
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn name(&self) -> &str {
        "browser_type"
    }
    fn toolset(&self) -> &str {
        "browser"
    }
    fn description(&self) -> &str {
        "Type text into an input or textarea by its ref ID (from browser_snapshot). \
         Pass {submit: true} to press Enter after typing. Returns element_stale envelope \
         if the ref no longer exists."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_type",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Element ref ID from browser_snapshot."
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type into the focused input."
                    },
                    "submit": {
                        "type": "boolean",
                        "description": "If true, press Enter after typing.",
                        "default": false
                    }
                },
                "required": ["ref", "text"]
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
        let ref_id = args
            .get("ref")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: ref (integer)"))?;

        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: text (string)"))?
            .to_string();

        let submit = args
            .get("submit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        debug!(ref_id, submit, text_len = text.len(), "browser_type");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        let selector = match sess.ref_table.get(&ref_id) {
            Some(s) => s.clone(),
            None => return Ok(element_stale(ref_id)),
        };

        // 1. Find element + focus.
        let el = match sess.page.find_element(&selector).await {
            Ok(el) => el,
            Err(e) => {
                debug!(error = %e, "find_element failed — element gone");
                return Ok(element_stale(ref_id));
            }
        };
        if let Err(e) = el.focus().await {
            debug!(error = %e, "focus failed");
            return Ok(element_stale(ref_id));
        }

        // 2. Clear existing value (select-all + delete via JS — robust across input types).
        let clear_js = format!(
            r#"(function() {{
                const el = document.querySelector({selector_lit});
                if (!el) return false;
                if ('value' in el) {{ el.value = ''; el.dispatchEvent(new Event('input', {{bubbles:true}})); }}
                else el.textContent = '';
                return true;
            }})()"#,
            selector_lit = serde_json::to_string(&selector).unwrap_or("\"\"".to_string())
        );
        if let Err(e) = sess.page.evaluate(clear_js.as_str()).await {
            debug!(error = %e, "clear-value failed; continuing with type");
        }

        // 3. Type the text via chromiumoxide.
        if let Err(e) = el.type_str(&text).await {
            debug!(error = %e, "type_str failed");
            return Ok(element_stale(ref_id));
        }

        // 4. Optional Enter press (form submit).
        let mut submitted = false;
        if submit {
            let press_js = format!(
                r#"(function() {{
                    const el = document.querySelector({selector_lit});
                    if (!el) return false;
                    const opts = {{ key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true }};
                    el.dispatchEvent(new KeyboardEvent('keydown', opts));
                    el.dispatchEvent(new KeyboardEvent('keypress', opts));
                    el.dispatchEvent(new KeyboardEvent('keyup', opts));
                    if (el.form && typeof el.form.requestSubmit === 'function') {{
                        try {{ el.form.requestSubmit(); }} catch(e) {{}}
                    }}
                    return true;
                }})()"#,
                selector_lit = serde_json::to_string(&selector).unwrap_or("\"\"".to_string())
            );
            if let Err(e) = sess.page.evaluate(press_js.as_str()).await {
                debug!(error = %e, "submit-enter dispatch failed; continuing");
            } else {
                submitted = true;
            }
        }

        Ok(json!({
            "typed": text,
            "ref": ref_id,
            "submitted": submitted
        })
        .to_string())
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
        let t = BrowserTypeTool::new(dummy_session());
        assert_eq!(t.name(), "browser_type");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn execute_rejects_missing_ref() {
        let t = BrowserTypeTool::new(dummy_session());
        let result = t.execute(json!({"text": "hi"})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing required parameter: ref")
        );
    }

    #[tokio::test]
    async fn execute_rejects_missing_text() {
        let t = BrowserTypeTool::new(dummy_session());
        let result = t.execute(json!({"ref": 5})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing required parameter: text")
        );
    }

    #[test]
    fn element_stale_envelope_shape_matches_d11() {
        let s = element_stale(3);
        assert!(s.contains("\"error\":\"element_stale\""));
        assert!(s.contains("\"ref\":3"));
    }
}
