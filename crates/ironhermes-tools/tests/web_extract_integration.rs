//! Phase 25.2 D-26 + D-27: wiremock-backed integration tests for web_extract.
//!
//! Tests in this file (added in plan 25.2-14):
//!   1. web_extract_single_url_local_fallback_returns_markdown      (D-26 #1, D-04)
//!   2. web_extract_pdf_url_routes_to_pdf_backend                   (D-26 #2, D-09)
//!   3. web_extract_youtube_url_dispatches_to_skill                 (D-26 #3, D-10)
//!   4. web_extract_summary_tier_thresholds                         (D-26 #4, D-11)
//!   5. web_extract_use_llm_processing_false_skips_all_aux_calls    (D-26 #5, D-12)
//!   6. web_extract_summarization_role_resolves_via_phase26_cascade (D-26 #6, D-13)
//!   7. web_extract_secret_in_url_redacted                          (D-26 #7, D-19)
//!   8. web_extract_multi_url_partial_failure_returns_per_url_errors (D-26 #8, D-02)
//!   9. web_extract_excluded_when_no_backend_available              (D-27 — schema-availability)
//!
//! All tests use wiremock (no live network). env_lock + EnvGuard are lifted from
//! crates/ironhermes-tools/tests/browser_integration.rs (Phase 25.1).

#![allow(dead_code)] // Helpers used by tests added in plan 25.2-14.

use std::sync::OnceLock;

/// Process-wide lock for env-var mutation in tests (Rust 2024 makes set_var unsafe).
/// Source: crates/ironhermes-tools/tests/browser_integration.rs:21
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// RAII guard that restores the previous env var value on drop.
/// Source: crates/ironhermes-tools/tests/browser_integration.rs:25-53
pub(crate) struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    pub(crate) fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: Rust 2024 set_var; tests serialised by env_lock().
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }

    pub(crate) fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: Rust 2024 remove_var; tests serialised by env_lock().
        unsafe { std::env::remove_var(key) };
        Self { key, prev: prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock(); restoring prior state.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
