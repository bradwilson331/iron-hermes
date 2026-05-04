//! Phase 25.1 D-04: browser_get_images — list all <img> elements on the page.
//!
//! Returns JSON array of {src, alt, ref, bbox}. Continues the ref-numbering past
//! browser_snapshot's last assigned ref (or starts from 1 if no snapshot was taken)
//! so the LLM can browser_click an image by ref.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{BrowserSession, find_chromium_binary};
use crate::registry::{Prerequisite, Tool};

const GET_IMAGES_JS: &str = r#"
(function(startCounter) {
    let counter = startCounter;
    const out = [];
    for (const img of document.querySelectorAll('img')) {
        const rect = img.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) continue;
        const style = window.getComputedStyle(img);
        if (style && (style.display === 'none' || style.visibility === 'hidden')) continue;
        let ref_ = img.getAttribute('data-ironhermes-ref');
        if (!ref_) {
            counter += 1;
            ref_ = String(counter);
            img.setAttribute('data-ironhermes-ref', ref_);
        }
        out.push({
            ref: parseInt(ref_, 10),
            src: img.getAttribute('src') || '',
            alt: img.getAttribute('alt') || '',
            bbox: { x: Math.round(rect.x), y: Math.round(rect.y),
                    w: Math.round(rect.width), h: Math.round(rect.height) }
        });
    }
    return { images: out, last_counter: counter };
})
"#;

pub struct BrowserGetImagesTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserGetImagesTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserGetImagesTool {
    fn name(&self) -> &str {
        "browser_get_images"
    }

    fn toolset(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "List all visible <img> elements on the current page with their src, alt, ref, and bounding box. \
         Refs continue the numbering from the last browser_snapshot call so you can browser_click an image by ref."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_get_images",
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
            description:
                "Chromium or Google Chrome browser binary on PATH or at a standard install location"
                    .to_string(),
            required: true,
        }]
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        debug!("browser_get_images invoked");
        let mut guard = self.session.lock().await;
        let sess = ensure_session(&mut guard).await?;

        // Continue numbering from the last ref assigned by snapshot (highest u64 in ref_table).
        let start_counter: u64 = sess.ref_table.keys().copied().max().unwrap_or(0);

        // page.evaluate doesn't accept arguments — embed the start_counter inline by wrapping the IIFE.
        let js = format!("({})({})", GET_IMAGES_JS.trim(), start_counter);

        let result = sess
            .page
            .evaluate(js.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("get_images failed: {e}"))?;

        let value: serde_json::Value = result
            .into_value()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let images = value
            .get("images")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        let last_counter = value
            .get("last_counter")
            .and_then(|v| v.as_u64())
            .unwrap_or(start_counter);

        // Register newly-assigned image refs in ref_table so browser_click can target them.
        if let Some(arr) = images.as_array() {
            for img in arr {
                if let Some(r) = img.get("ref").and_then(|v| v.as_u64()) {
                    if r > start_counter {
                        sess.ref_table
                            .insert(r, format!("[data-ironhermes-ref=\"{r}\"]"));
                    }
                }
            }
        }

        let _ = last_counter; // for tracing if needed
        Ok(json!({ "images": images }).to_string())
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
        let t = BrowserGetImagesTool::new(dummy_session());
        assert_eq!(t.name(), "browser_get_images");
        assert_eq!(t.toolset(), "browser");
    }

    #[test]
    fn get_images_js_skips_zero_sized_and_invisible() {
        // Static invariant — JS must check rect.width/height + display/visibility.
        assert!(GET_IMAGES_JS.contains("rect.width === 0"));
        assert!(GET_IMAGES_JS.contains("display === 'none'"));
    }

    #[test]
    fn get_images_js_assigns_ref_continuation() {
        // Static invariant — JS uses startCounter argument and assigns data-ironhermes-ref.
        assert!(GET_IMAGES_JS.contains("startCounter"));
        assert!(GET_IMAGES_JS.contains("data-ironhermes-ref"));
    }
}
