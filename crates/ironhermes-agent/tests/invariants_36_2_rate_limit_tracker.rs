//! Phase 36.2 Plan 06: static-grep invariants for `rate_limit_tracker.rs`.
//!
//! Locks the D-RL-03 (no-fallback-import) and security (no-cleartext-key)
//! contracts as include_str! grep gates.

const TRACKER_SOURCE: &str = include_str!("../src/rate_limit_tracker.rs");

fn non_comment_lines() -> String {
    TRACKER_SOURCE
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && !t.starts_with("///")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tracker_has_struct() {
    assert!(
        TRACKER_SOURCE.contains("pub struct RateLimitTracker"),
        "rate_limit_tracker.rs must declare pub struct RateLimitTracker"
    );
}

#[test]
fn tracker_no_fallback_providers_import() {
    assert!(
        !non_comment_lines().contains("fallback_providers"),
        "D-RL-03: tracker is pure observability, must not reference fallback_providers"
    );
}

#[test]
fn tracker_no_classify_llm_error_call() {
    assert!(
        !non_comment_lines().contains("classify_llm_error"),
        "D-RL-03: tracker must not call classify_llm_error"
    );
}

#[test]
fn tracker_uses_sha2_not_blake3() {
    let non_comment = non_comment_lines();
    assert!(
        !non_comment.contains("blake3"),
        "RESEARCH.md correction #5: blake3 not in Cargo.lock, must use sha2"
    );
    assert!(
        TRACKER_SOURCE.contains("use sha2::"),
        "tracker must import from sha2 (SHA-256-truncated-16-bytes per D-RL-02)"
    );
}

#[test]
fn tracker_parses_anthropic_prefix() {
    assert!(
        TRACKER_SOURCE.contains("anthropic-ratelimit-"),
        "tracker must parse the anthropic-ratelimit-* header prefix (RESEARCH correction #1)"
    );
}

#[test]
fn tracker_parses_x_ratelimit_prefix() {
    assert!(
        TRACKER_SOURCE.contains("x-ratelimit-"),
        "tracker must parse the x-ratelimit-* header prefix (OpenRouter / Nous)"
    );
}

#[test]
fn tracker_no_cleartext_key_literal() {
    assert!(
        !non_comment_lines().contains("\"sk-"),
        "no cleartext-key-shaped literal (\"sk-…\") may appear in tracker source"
    );
}

#[test]
fn tracker_has_hash_api_key_function() {
    assert!(
        TRACKER_SOURCE.contains("pub fn hash_api_key(key: &str) -> [u8; 16]"),
        "tracker must expose pub fn hash_api_key(&str) -> [u8; 16]"
    );
}
