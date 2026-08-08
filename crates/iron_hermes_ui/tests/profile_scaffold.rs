//! Phase 47.4 Plan 03 Task 3 — `profile_api.rs`'s `create_profile` shape
//! locks + registration checks.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot reach `pub(crate)` items in
//! `src/server/profile_api.rs` — the identical, already-established
//! constraint `tests/profile_health.rs` documents for Plan 01's helpers
//! (see that file's own module doc, and its "Note on the plan's literal
//! acceptance-criteria greps" in `.planning/.../47.4-01-SUMMARY.md`). The
//! real fixture-directory D-08/D-06/D-07/D-13 functional tests for
//! `create_profile_impl` / `resolve_inherited_keys` / `mask_key_value` /
//! `write_env_atomic_0600` / `check_profile_write_gate` — every case in
//! the plan's `<behavior>` block, asserted against real files on disk —
//! live as `#[cfg(test)]` unit tests inside `src/server/profile_api.rs`
//! itself (`mod profile_scaffold_tests`), where `pub(crate)` visibility
//! actually reaches them. Run them with:
//!
//!   cargo nextest run -p iron_hermes_ui --features server profile_scaffold_tests --test-threads=1
//!
//! This file only locks the module's *shape* — the fn/const names, the
//! defense-in-depth reuse of `validate_profile_name`, the write gate, and
//! the atomic-0600 idiom's signature markers — via source-string
//! assertions, mirroring `tests/profile_health.rs` / `tests/provider_config_api.rs`.
//! (The plan's own literal automated verify command,
//! `cargo nextest run -p iron_hermes_ui profile_scaffold --test-threads=1`
//! with no `--features server`, does not build on its own — this crate's
//! `main.rs` unconditionally references `#[cfg(feature = "server")]`-gated
//! modules regardless of feature selection, same finding as Plan 01's
//! SUMMARY documents for `profile_health`; the corrected invocation is
//! documented above.)

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn profile_api_declares_create_profile() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("pub async fn create_profile("),
        "profile_api.rs must declare `pub async fn create_profile(`"
    );
    assert!(
        src.contains("#[server]\npub async fn create_profile("),
        "create_profile must be a #[server] fn"
    );
}

#[test]
fn create_profile_reuses_validate_profile_name_not_a_reimplementation() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("ironhermes_core::profile::validate_profile_name"),
        "create_profile's scaffold must call the real validate_profile_name, \
         not re-implement the rules (D-08)"
    );
}

#[test]
fn create_profile_gates_on_web_config_write_enabled() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("web_config_write_enabled"),
        "create_profile must gate on config.security.web_config_write_enabled, \
         matching update_provider_config/write_provider_secret (D-06)"
    );
}

#[test]
fn create_profile_writes_no_field_named_value_into_key_row_dto() {
    // Redundant with protocol_key_row test in profile_health.rs, but keeps
    // this file self-contained for the D-13 no-value-field invariant this
    // plan's scaffold depends on.
    let src = read("src/protocol.rs");
    let start = src
        .find("pub struct KeyRow {")
        .expect("KeyRow must be declared");
    let end = src[start..]
        .find('}')
        .map(|i| start + i)
        .expect("KeyRow struct body must close");
    let body = &src[start..end];
    assert!(!body.contains("value"));
}

/// Isolates the production-code slice of `profile_api.rs`, excluding BOTH
/// `#[cfg(test)]` modules (`profile_health_tests` from Plan 01,
/// `profile_scaffold_tests` from this plan) — both legitimately contain
/// `unwrap()`/`expect()` (test assertions) and `env::set_var` (the
/// `ScopedEnv` guard itself, needed to exercise `IRONHERMES_HOME`-scoped
/// fns), so a whole-file grep would false-positive against the tests it
/// protects. `profile_scaffold_tests` is declared after
/// `profile_health_tests` in the source file, so slicing at the first
/// module's boundary excludes both.
fn profile_api_production_src() -> String {
    let src = read("src/server/profile_api.rs");
    let boundary = "mod profile_health_tests";
    match src.find(boundary) {
        Some(idx) => src[..idx].to_string(),
        None => src,
    }
}

/// D-08: no subprocess anywhere in this file's production code — the only
/// `Command::new` call sites in this server are `ffmpeg` in `server/ws.rs`.
#[test]
fn profile_api_never_shells_out_in_production_code() {
    let src = profile_api_production_src();
    assert_eq!(
        src.matches("Command::new").count(),
        0,
        "profile_api.rs production code must never shell out to a subprocess (D-08)"
    );
}

/// T-47.4-03-T1/E1: no process-env mutation, no force-unwrap, anywhere in
/// this file's production code.
#[test]
fn profile_api_never_mutates_env_or_force_unwraps_in_production_code() {
    let src = profile_api_production_src();
    assert_eq!(
        src.matches("env::set_var").count(),
        0,
        "profile_api.rs production code must never mutate the process environment"
    );
    assert_eq!(
        src.matches("unwrap()").count(),
        0,
        "profile_api.rs production code must contain zero occurrences of `unwrap()`"
    );
}

/// D-06: no per-profile secret-storage path anywhere in this file — not a
/// namespaced write, not even a mention of the word. Whole-file check (not
/// scoped to production code) since the tests must not reference it either.
#[test]
fn profile_api_never_mentions_vault_storage() {
    let src = read("src/server/profile_api.rs");
    assert_eq!(
        src.matches("vault").count(),
        0,
        "profile_api.rs must never reference vault storage — D-06 keeps keys \
         in the plaintext profile .env only"
    );
}

/// T-47.4-03-I2: the atomic-0600 idiom's signature markers are present —
/// the temp-file open mode plus the redundant final chmod.
#[test]
fn write_env_atomic_uses_0600_at_least_twice() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.matches("0o600").count() >= 2,
        "expected at least 2 occurrences of 0o600 (temp open mode + final set_permissions)"
    );
}

#[test]
fn write_env_atomic_uses_create_new() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("create_new(true)"),
        "the atomic .env write must use OpenOptions::create_new(true), \
         matching AuthStore::save_to_disk"
    );
}

/// The resolved D-07 checkpoint (`proceed-with-inventory`) requires a
/// machine-readable provenance stamp on every generated `.env`.
#[test]
fn profile_env_provenance_prefix_is_declared_and_used() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("PROFILE_ENV_PROVENANCE_PREFIX"),
        "the resolved checkpoint requires a stable, greppable provenance \
         constant stamped into every generated profile .env"
    );
}

/// Phase 47.4 Plan 20 (CR-03/CR-04) shape-locks. All three strip `//`
/// comment lines first — the module doc, the design-decision block, and
/// several test doc comments legitimately discuss the OLD unquoted
/// behavior and the fn names by full text, so a whole-file grep would be
/// self-invalidating (the same reasoning `profile_verify_classification.rs`
/// documents for its own CR-01 shape-lock).
fn strip_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A failed self-check round-trip must never reach `write_env_atomic_0600`
/// — `render_profile_env`'s signature is the compiler-enforced shape-lock
/// for that (D-06/D-07).
#[test]
fn render_profile_env_is_fallible_so_a_failed_round_trip_cannot_reach_disk() {
    let stripped = strip_comment_lines(&read("src/server/profile_api.rs"));
    assert!(
        stripped.contains(
            "fn render_profile_env(\n    name: &str,\n    entries: &[(String, String)],\n) -> Result<String, String>"
        ) || stripped.contains(
            "fn render_profile_env(name: &str, entries: &[(String, String)]) -> Result<String, String>"
        ),
        "render_profile_env must be declared returning Result<String, String>, so a \
         failed self-check refuses the write (CR-03/CR-04)"
    );
}

/// The self-check that makes this the last round of this bug class: the
/// renderer proves its own output against the REAL parser before returning.
///
/// Phase 47.6 Plan 04 (D-06): `verify_render_round_trip`'s own round-trip
/// call to `dotenvy::from_read_iter` was hoisted into
/// `ironhermes_core::dotenv_write::verify_env_round_trip` — `profile_api.rs`
/// now delegates to it rather than reimplementing it locally, so this test
/// checks both halves of that delegation: `profile_api.rs` calls the real
/// shared verifier (not a hand-rolled reimplementation of its own), and that
/// shared verifier is itself backed by the real `dotenvy::from_read_iter`.
#[test]
fn render_profile_env_verifies_its_own_output_against_the_real_parser() {
    let stripped = strip_comment_lines(&read("src/server/profile_api.rs"));
    assert!(
        stripped.contains("ironhermes_core::dotenv_write::verify_env_round_trip"),
        "verify_render_round_trip must delegate to the single shared \
         ironhermes_core::dotenv_write::verify_env_round_trip, not reimplement \
         the round-trip check locally (Phase 47.6 Plan 04, D-06)"
    );

    let core_src = fs::read_to_string(
        crate_root()
            .join("../ironhermes-core/src/dotenv_write.rs")
            .canonicalize()
            .expect("ironhermes-core/src/dotenv_write.rs must exist (Phase 47.6 Plan 04 hoist)"),
    )
    .expect("failed to read ironhermes-core/src/dotenv_write.rs");
    let stripped_core = strip_comment_lines(&core_src);
    assert!(
        stripped_core.contains("dotenvy::from_read_iter"),
        "the shared verify_env_round_trip must round-trip its rendered bytes \
         through the real dotenvy::from_read_iter, not a hand-rolled reimplementation"
    );
}

/// Pins both production call sites to propagate the fallible render result
/// rather than unwrap it, so a self-check failure always refuses the write.
#[test]
fn both_write_paths_propagate_the_render_result() {
    let stripped = strip_comment_lines(&profile_api_production_src());

    let call_sites: Vec<&str> = stripped
        .lines()
        .filter(|line| line.contains("render_profile_env(") && !line.contains("fn render_profile_env("))
        .collect();

    assert_eq!(
        call_sites.len(),
        2,
        "expected exactly 2 production call sites of render_profile_env(, found {}: {call_sites:?}",
        call_sites.len()
    );

    for line in &call_sites {
        assert!(
            !line.contains("unwrap()"),
            "a render_profile_env( call site must not unwrap its Result: {line}"
        );
        assert!(
            line.trim_end().ends_with(")?;"),
            "a render_profile_env( call site must propagate its Result with `?`: {line}"
        );
    }
}
