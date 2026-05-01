//! Phase 25.1 D-04 / D-03: browser_close — explicit session teardown.
//!
//! Idempotent: calling close on a None-state session is a no-op.
//! After close, the Arc<Mutex<Option<...>>> is left at None, so the next
//! browser_* call lazy-respawns (D-03 contract).
//!
//! Threat anchor T-25.1-04: explicit teardown aborts the CDP handler_task
//! and chromium exits cleanly via Browser::close().

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

pub struct BrowserCloseTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
}

impl BrowserCloseTool {
    pub fn new(session: Arc<Mutex<Option<BrowserSession>>>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for BrowserCloseTool {
    fn name(&self) -> &str { "browser_close" }
    fn toolset(&self) -> &str { "browser" }
    fn description(&self) -> &str {
        "Close the browser session and free chromium resources. \
         The next browser_* call will lazy-respawn a new session."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_close",
            self.description(),
            json!({
                "type": "object",
                "properties": {},
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

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        debug!("browser_close invoked");
        let mut guard = self.session.lock().await;
        let was_active = guard.is_some();

        if let Some(sess) = guard.take() {
            // Best-effort teardown — log errors but don't propagate.
            // T-25.1-04 mitigation: handler_task is .abort()'d inside close().
            if let Err(e) = sess.close().await {
                warn!(error = %e, "browser_close: close() returned error (resources still released via Drop)");
            }
        }

        // After this, *guard is None — D-03 next-call-respawns contract.
        Ok(json!({ "closed": true, "was_active": was_active }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_session() -> Arc<Mutex<Option<BrowserSession>>> {
        Arc::new(Mutex::new(None))
    }

    #[test]
    fn name_and_toolset_match_d04() {
        let t = BrowserCloseTool::new(dummy_session());
        assert_eq!(t.name(), "browser_close");
        assert_eq!(t.toolset(), "browser");
    }

    #[tokio::test]
    async fn close_on_none_session_is_idempotent() {
        let session = dummy_session();
        let t = BrowserCloseTool::new(session.clone());
        let result = t.execute(json!({})).await.unwrap();
        assert!(result.contains("\"closed\":true"));
        assert!(result.contains("\"was_active\":false"));
        // Verify guard is still None.
        let guard = session.lock().await;
        assert!(guard.is_none(), "after close on None, session must remain None for next-call respawn");
    }
}
