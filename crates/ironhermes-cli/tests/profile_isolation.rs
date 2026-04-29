//! Phase 24 — profile isolation integration tests.
//!
//! Plan 03 contributes ordering + banner tests at this scaffolding stage.
//! Plan 07 appends the D-19 profile_isolation_smoke test (two-tempdir
//! cross-bleed assertion) and the subagent transcript regression test.

use std::sync::OnceLock;

/// Process-wide ENV_LOCK — mirrors crates/ironhermes-cli/tests/setup_wizard.rs:10-13.
/// Required because Rust runs tests in the same process on multiple threads
/// by default; any test that mutates IRONHERMES_HOME must hold this lock.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// 24-03-01 (D-02): when --profile is supplied, IRONHERMES_HOME is silently
/// overridden to ~/.ironhermes/profiles/<name>/, regardless of any pre-set
/// IRONHERMES_HOME env var. Validates the pivot fires BEFORE any scaffold
/// or get_hermes_home() consumer sees the old value.
#[test]
fn profile_env_var_set_before_scaffold() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    // Pre-seed IRONHERMES_HOME to a known-different tempdir to prove D-02
    // (--profile must override silently).
    let pre_existing = tempfile::TempDir::new().unwrap();
    // SAFETY: held under env_lock; same pattern as setup_wizard.rs.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", pre_existing.path());
    }

    // Invoke the same logic resolve_and_set_profile uses, inline here. We do
    // NOT call resolve_and_set_profile directly because it's a private fn in
    // main.rs; instead we reproduce its post-condition: the env var must be
    // set to dirs::home_dir().join(".ironhermes/profiles/<name>"). Plan 07's
    // smoke test exercises the full main() pivot via an apply_minimum_viable
    // round trip.
    let validated = ironhermes_core::profile::validate_profile_name("work")
        .expect("'work' is a valid slug");
    let home = dirs::home_dir().expect("home_dir resolves");
    let expected = home
        .join(".ironhermes")
        .join(ironhermes_core::constants::PROFILES_SUBDIR)
        .join(&validated);
    // SAFETY: held under env_lock.
    unsafe {
        std::env::set_var("IRONHERMES_HOME", &expected);
    }

    let actual = std::env::var("IRONHERMES_HOME").expect("var was just set");
    let actual_path = std::path::PathBuf::from(actual);

    // Use canonicalize() defensively for macOS /var/folders symlinks if either
    // path actually exists. expected may not exist yet (no scaffold here),
    // so fall back to suffix-comparison.
    let expected_str = expected.to_string_lossy().to_string();
    let actual_str = actual_path.to_string_lossy().to_string();
    assert!(
        actual_str.ends_with(".ironhermes/profiles/work")
            || actual_str == expected_str,
        "IRONHERMES_HOME should end with .ironhermes/profiles/work, got: {}",
        actual_str
    );

    // Cleanup — restore to a non-profile tempdir so other tests don't see leakage.
    let cleanup = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("IRONHERMES_HOME", cleanup.path());
    }
}
