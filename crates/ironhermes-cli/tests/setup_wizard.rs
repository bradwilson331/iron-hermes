//! Integration tests for `hermes setup` wizard and preflight middleware.
//! Phase 23 Plan 02 — Tasks 2, 3, 7.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Scaffold placeholder — real tests added in Tasks 2/3/7.
#[test]
fn scaffold_placeholder() {
    assert!(true, "scaffold");
}
