//! Phase 47.4 Plan 06 Task 3 — `profile_verify_api.rs`'s `verify_profile`
//! shape locks + registration checks.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot reach `pub(crate)` items in
//! `src/server/profile_verify_api.rs` — the identical, already-established
//! constraint `tests/profile_health.rs` / `tests/profile_scaffold.rs`
//! document for Plans 01/03's helpers. The real fixture-directory D-09
//! functional tests for `run_disk_checks` / `classify_probe_result` /
//! `summarize_probe_error` — every case in the plan's `<behavior>` block,
//! asserted against real fixture directories and real function calls — live
//! as `#[cfg(test)]` unit tests inside `src/server/profile_verify_api.rs`
//! itself (`mod profile_verify_classification_tests` +
//! `mod profile_verify_disk_checks_tests`), where `pub(crate)` visibility
//! actually reaches them. Run them with:
//!
//!   cargo nextest run -p iron_hermes_ui --features server profile_verify_classification_tests --test-threads=1
//!   cargo nextest run -p iron_hermes_ui --features server profile_verify_disk_checks_tests --test-threads=1
//!
//! This file only locks the module's *shape* — the fn/const names, the
//! env-override wiring, the timeout, and the no-mock/no-bridge invariants —
//! via source-string assertions, mirroring `tests/profile_health.rs` /
//! `tests/profile_scaffold.rs` / `tests/provider_config_api.rs`. (The plan's
//! own literal automated verify command,
//! `cargo nextest run -p iron_hermes_ui profile_verify_classification --test-threads=1`
//! with no `--features server`, does not build on its own — this crate's
//! `main.rs` unconditionally references `#[cfg(feature = "server")]`-gated
//! modules regardless of feature selection, the same finding Plans 01/03's
//! SUMMARYs document; the corrected invocations are above.)
//!
//! ## What this file does NOT cover, and why
//!
//! The real network path — `verify_profile` actually reaching a provider
//! through the judge closure it builds — is **manual-UAT-only by design**
//! (`47.4-VALIDATION.md` § Manual-Only Verifications). A mocked `JudgeFn`
//! that returns success here would prove nothing about D-09 and would be
//! exactly the self-verifying-mock failure this project has shipped before
//! (46.7 UAT; the venice video mocks — see project memory
//! `feedback_tests_that_verify_their_own_assumptions`). No test in this
//! crate, in this file or in `profile_verify_api.rs`'s own test modules,
//! constructs a fake `JudgeFn` closure that returns success and asserts the
//! probe reports `Success`.
//!
//! The manual procedure lives in the plan's `<human-check>` (Task 2) and in
//! `47.4-VALIDATION.md`. Its case 3 is the load-bearing one: with
//! `dx serve --package iron_hermes_ui` running, verify a profile whose
//! `.env` is **empty** while the **server process** has a valid root key in
//! its own environment — the expected result is `Failure`, never `Success`.
//! A `Success` there would mean the probe is reading the root key instead of
//! the target profile's own `.env`, which is exactly the false-confidence
//! failure D-09/D-14 exist to prevent. This is the assertion that actually
//! proves the D-14 override map is doing its job — no automated test can
//! stand in for it, because standing in for it is precisely the failure
//! mode being guarded against.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn profile_verify_api_declares_verify_profile() {
    let src = read("src/server/profile_verify_api.rs");
    assert!(
        src.contains("pub async fn verify_profile("),
        "profile_verify_api.rs must declare `pub async fn verify_profile(`"
    );
    assert!(
        src.contains("#[server]\npub async fn verify_profile("),
        "verify_profile must be a #[server] fn"
    );
}

#[test]
fn profile_verify_api_registered_in_server_mod() {
    let src = read("src/server/mod.rs");
    assert!(
        src.contains("pub mod profile_verify_api;"),
        "server/mod.rs must register `pub mod profile_verify_api;` so \
         register_server_functions() (and therefore require_auth) reaches it"
    );
}

#[test]
fn verify_profile_uses_the_env_override_judge_builder_not_the_root_scoped_one() {
    let src = read("src/server/profile_verify_api.rs");
    assert!(
        src.contains("build_runtime_judge_fn_with_env_overrides"),
        "verify_profile's probe must be built via the D-14 override-taking sibling"
    );
    assert_eq!(
        src.matches("build_runtime_judge_fn(").count(),
        0,
        "the non-override judge builder reads the server process's own \
         environment — it must never appear in this file"
    );
}

#[test]
fn verify_profile_wraps_the_judge_call_in_a_bounded_timeout() {
    let src = read("src/server/profile_verify_api.rs");
    assert_eq!(
        src.matches("tokio::time::timeout").count(),
        1,
        "the judge probe call must be wrapped in exactly one bounded timeout"
    );
}

#[test]
fn verify_profile_never_bridges_through_the_unsafe_in_place_blocking_helper() {
    let src = read("src/server/profile_verify_api.rs");
    assert_eq!(
        src.matches("block_in_place").count(),
        0,
        "structuring disk work in spawn_blocking and awaiting the judge \
         future directly must avoid this bridge entirely"
    );
}

/// Isolates the production-code slice of `profile_verify_api.rs`, excluding
/// every `#[cfg(test)]` module — both legitimately contain `expect()` (test
/// assertions) and `env::set_var` (the `ScopedEnv` guard itself, needed to
/// exercise `IRONHERMES_HOME`-scoped fns), so a whole-file grep would
/// false-positive against the tests it protects.
fn profile_verify_api_production_src() -> String {
    let src = read("src/server/profile_verify_api.rs");
    let boundary = "mod profile_verify_classification_tests";
    match src.find(boundary) {
        Some(idx) => src[..idx].to_string(),
        None => src,
    }
}

#[test]
fn profile_verify_api_never_mutates_the_process_environment_in_production_code() {
    let src = profile_verify_api_production_src();
    assert_eq!(
        src.matches("env::set_var").count(),
        0,
        "profile_verify_api.rs production code must never mutate the process \
         environment — profile scoping happens entirely through the D-14 \
         override map"
    );
}

#[test]
fn profile_verify_api_never_shells_out_in_production_code() {
    let src = profile_verify_api_production_src();
    assert_eq!(
        src.matches("Command::new").count(),
        0,
        "profile_verify_api.rs production code must never shell out to a subprocess"
    );
}

#[test]
fn profile_verify_api_never_force_unwraps_in_production_code() {
    let src = profile_verify_api_production_src();
    assert_eq!(
        src.matches("unwrap()").count(),
        0,
        "profile_verify_api.rs production code must contain zero occurrences of `unwrap()`"
    );
}

#[test]
fn profile_verify_api_declares_run_disk_checks_and_classification_helpers() {
    let src = read("src/server/profile_verify_api.rs");
    for needle in [
        "fn run_disk_checks",
        "fn classify_probe_result",
        "fn summarize_probe_error",
    ] {
        assert!(
            src.contains(needle),
            "profile_verify_api.rs must declare `{needle}`"
        );
    }
}

#[test]
fn summarize_probe_error_uses_char_boundary_safe_truncation() {
    let src = read("src/server/profile_verify_api.rs");
    assert!(
        src.contains("chars().take("),
        "the error-summary truncation must be char-boundary-safe, never a byte slice"
    );
}

#[test]
fn verify_profile_never_wired_into_profile_api_enumeration() {
    let src = read("src/server/profile_api.rs");
    assert_eq!(
        src.matches("verify_profile").count(),
        0,
        "D-11: the real probe must never be called from list_profiles, a \
         dropdown open handler, or any per-row render"
    );
}

/// Phase 47.4 Plan 17 (CR-01) shape-lock: pins `build_probe_setup`'s two
/// judge-cascade call sites to the `_strict` siblings against a fifth
/// recurrence of this bug class (GAP-1 → GAP-7 → GAP-8 → CR-01). Comment
/// stripping is mandatory and load-bearing: the module doc legitimately
/// discusses the permissive resolver names by full text (the CR-01 history
/// this task adds), and a whole-file grep would make the doc
/// self-invalidating.
#[test]
fn verify_probe_never_calls_the_process_env_falling_back_judge_resolvers() {
    let src = profile_verify_api_production_src();
    let stripped: String = src
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    // Each search string includes the trailing `(` so a `_strict` sibling's
    // longer name (which continues past `overrides` with `_strict(`, never
    // `(`) can never satisfy the permissive needle — the delimiter alone
    // enforces "name immediately followed by an open parenthesis".
    let permissive_calls = [
        "resolve_judge_model_and_endpoint_with_env_overrides(",
        "build_runtime_judge_fn_with_env_overrides(",
    ];
    let strict_calls = [
        "resolve_judge_model_and_endpoint_with_env_overrides_strict(",
        "build_runtime_judge_fn_with_env_overrides_strict(",
    ];

    for name in permissive_calls {
        let count = stripped.matches(name).count();
        assert_eq!(
            count, 0,
            "CR-01 shape-lock: `{name}` must never be CALLED from build_probe_setup \
             (process-environment fallback would answer the server's question, not the \
             spawned worker's) — found {count} occurrence(s) after stripping comment lines"
        );
    }
    for name in strict_calls {
        let count = stripped.matches(name).count();
        assert!(
            count >= 1,
            "CR-01 shape-lock: `{name}` must be called at least once — proves the strict \
             path is present, not merely that the permissive one is absent; found {count}"
        );
    }
}

#[test]
fn ironhermes_kanban_crate_untouched_by_this_plan() {
    // A source-level guard mirroring the plan's `git diff --stat
    // crates/ironhermes-kanban/` acceptance criterion: this file must never
    // itself declare judge-building logic that could tempt a future editor
    // to relocate it into the fenced crate.
    let src = read("src/server/profile_verify_api.rs");
    assert!(
        !src.contains("ironhermes_kanban::judge::"),
        "no judge-building logic may be added to or relocated into \
         crates/ironhermes-kanban/ (the locked crate-isolation fence)"
    );
}
