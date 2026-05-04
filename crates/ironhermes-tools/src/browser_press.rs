//! Phase 25.1 D-04: browser_press — synthesize a keyboard event in the page.
//!
//! Dispatches a synthetic KeyboardEvent via page.evaluate. Names follow standard
//! `KeyboardEvent.key`: "Enter", "Tab", "Escape", "ArrowDown", "a", "A", etc.
//! Optional modifiers: ["ctrl", "shift", "alt", "meta"].

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{BrowserSession, find_chromium_binary};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserPressTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserPressTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserPressTool {
    fn name(&self) -> &str {
        "browser_press"
    }
    fn toolset(&self) -> &str {
        "browser"
    }
    fn description(&self) -> &str {
        "Press a keyboard key in the page. Names match standard JS KeyboardEvent.key (e.g. 'Enter', 'Tab', 'Escape', 'ArrowDown', 'a'). Optional modifiers: ['ctrl', 'shift', 'alt', 'meta']."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_press",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The key to press (e.g. 'Enter', 'Tab', 'Escape', 'ArrowDown', 'a')."
                    },
                    "modifiers": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["ctrl", "shift", "alt", "meta"] },
                        "description": "Optional modifier keys held during the press.",
                        "default": []
                    }
                },
                "required": ["key"]
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
        let key = args["key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: key"))?;

        let modifiers: Vec<String> = args
            .get("modifiers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let ctrl = modifiers.iter().any(|m| m == "ctrl");
        let shift = modifiers.iter().any(|m| m == "shift");
        let alt = modifiers.iter().any(|m| m == "alt");
        let meta = modifiers.iter().any(|m| m == "meta");

        debug!(key, ctrl, shift, alt, meta, "browser_press");

        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // RESEARCH OQ-3-style: JS KeyboardEvent dispatch — CDP-version-agnostic.
        // Embedding `key` directly is safe because we JSON-escape it.
        let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
        let js = format!(
            r#"(function() {{
                const target = document.activeElement || document.body;
                const opts = {{ key: {key_json}, bubbles: true, cancelable: true,
                                ctrlKey: {ctrl}, shiftKey: {shift}, altKey: {alt}, metaKey: {meta} }};
                target.dispatchEvent(new KeyboardEvent('keydown', opts));
                target.dispatchEvent(new KeyboardEvent('keypress', opts));
                target.dispatchEvent(new KeyboardEvent('keyup', opts));
                return true;
            }})()"#,
            key_json = key_json,
            ctrl = ctrl,
            shift = shift,
            alt = alt,
            meta = meta
        );

        sess.page
            .evaluate(js.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("press failed: {e}"))?;

        Ok(json!({ "pressed": key, "modifiers": modifiers }).to_string())
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
        let t = BrowserPressTool::new(dummy_session());
        assert_eq!(t.name(), "browser_press");
        assert_eq!(t.toolset(), "browser");
    }

    #[test]
    fn schema_requires_key_arg() {
        let t = BrowserPressTool::new(dummy_session());
        let s = t.schema();
        let required = s
            .function
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            required.iter().any(|v| v.as_str() == Some("key")),
            "key MUST be required"
        );
    }

    #[tokio::test]
    async fn execute_rejects_missing_key() {
        let t = BrowserPressTool::new(dummy_session());
        let result = t.execute(json!({})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing required parameter: key")
        );
    }
}
