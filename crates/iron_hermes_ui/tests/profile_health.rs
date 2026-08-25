//! Phase 47.4 Plan 01 Task 3 — `profile_api.rs` shape locks + registration
//! checks.
//!
//! `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`), so integration
//! tests under `tests/` cannot `use iron_hermes_ui::...` (see
//! `tests/provider_config_api.rs`'s own doc comment for the identical,
//! already-established constraint in this codebase). The real
//! fixture-directory D-11 functional tests — `classify_profile_health`
//! across every permutation, `read_env_keys` against real 0600 `.env`
//! fixtures via `tempfile::tempdir()` + the `ScopedEnv` `IRONHERMES_HOME`
//! guard (copied from `ironhermes-kanban/src/paths.rs:449-475`), and the
//! traversal/dotfile/sort enumeration cases — live as `#[cfg(test)]` unit
//! tests inside `src/server/profile_api.rs` itself (`mod
//! profile_health_tests`), where `pub(crate)` visibility actually reaches
//! them. Run them with:
//!
//!   cargo nextest run -p iron_hermes_ui --features server profile_health --test-threads=1
//!
//! This file only locks the module's *shape* — the fn/const names and the
//! `server/mod.rs` registration line — via source-string assertions,
//! mirroring `tests/provider_config_api.rs` / `tests/kanban_server_fns.rs`.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn profile_api_declares_list_profiles() {
    let src = read("src/server/profile_api.rs");
    assert!(
        src.contains("pub async fn list_profiles("),
        "profile_api.rs must declare `pub async fn list_profiles(`"
    );
    assert!(
        src.contains("#[server]"),
        "list_profiles must be a #[server] fn"
    );
}

#[test]
fn profile_api_module_is_registered() {
    let src = read("src/server/mod.rs");
    assert!(
        src.contains("pub mod profile_api;"),
        "server/mod.rs must contain `pub mod profile_api;`"
    );
}

#[test]
fn profile_switcher_module_is_registered() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        src.contains("pub mod profile_switcher;"),
        "kanban.rs must contain `pub mod profile_switcher;`"
    );
}

/// Isolates the production-code slice of `profile_api.rs`, excluding the
/// `profile_health_tests` module — that module legitimately contains
/// `unwrap()` (test assertions) and `env::set_var` (the `ScopedEnv` guard
/// itself, needed to exercise `IRONHERMES_HOME`-dependent fns), so a
/// whole-file grep would false-positive against the tests it protects.
fn profile_api_production_src() -> String {
    let src = read("src/server/profile_api.rs");
    let boundary = "mod profile_health_tests";
    match src.find(boundary) {
        Some(idx) => src[..idx].to_string(),
        None => src,
    }
}

/// T-47.4-01-D1: `read_env_keys` must never force-unwrap — every error is
/// propagated as a value, checked here as a standing zero-occurrence gate
/// on the production-code slice (matches the plan's acceptance criteria).
#[test]
fn profile_api_never_force_unwraps() {
    let src = profile_api_production_src();
    assert_eq!(
        src.matches("unwrap()").count(),
        0,
        "profile_api.rs production code must contain zero occurrences of `unwrap()`"
    );
}

/// T-47.4-01-T1 / T-47.4-01-E1: no process-env mutation, no subprocess
/// spawn, anywhere in this file's production code.
#[test]
fn profile_api_never_mutates_env_or_shells_out() {
    let src = profile_api_production_src();
    assert_eq!(
        src.matches("env::set_var").count(),
        0,
        "profile_api.rs production code must never mutate the process environment"
    );
    assert_eq!(
        src.matches("Command::new").count(),
        0,
        "profile_api.rs production code must never shell out to a subprocess"
    );
}

#[test]
fn protocol_declares_the_full_profile_dto_contract() {
    let src = read("src/protocol.rs");
    for name in [
        "pub struct ProfileRow",
        "pub struct KeyRow",
        "pub struct ProfileDetail",
        "pub enum VerifyOutcome",
        "pub struct VerifyReport",
        "pub struct CreateProfileRequest",
    ] {
        assert!(
            src.contains(name),
            "protocol.rs must declare `{name}`"
        );
    }
}

/// Phase 47.4 Plan 11 (GAP-1): the create wizard's client-only
/// `CLIENT_LLM_KEY_ALLOWLIST` (a wasm-target literal — it cannot call the
/// server's provider-registry-derived `provider_key_env_names`) must remain
/// a SUBSET of `profile_api.rs`'s `LLM_KEY_ALLOWLIST` compatibility floor —
/// itself always a subset of the server's authoritative derived set (see
/// `derived_key_names_retain_legacy_five_floor` in `profile_api.rs`'s own
/// `#[cfg(test)]` module). A source-string assertion, mirroring this file's
/// own established pattern, since integration tests here cannot reach
/// either `pub(crate)` constant directly (this file's own module doc).
///
/// Phase 50.1 Plan 02 (D-10): the declaration relocated from
/// `screens/kanban/wizard.rs` (now a thin re-export shim) to
/// `screens/profile_shared/create_dialog.rs` — this test reads the new
/// location.
#[test]
fn client_llm_key_allowlist_is_a_subset_of_the_server_floor() {
    let client_src = read("src/components/hermes_app/screens/profile_shared/create_dialog.rs");
    let client_start = client_src
        .find("const CLIENT_LLM_KEY_ALLOWLIST: [&str; 5] = [")
        .expect("CLIENT_LLM_KEY_ALLOWLIST must be declared in wizard.rs");
    let client_end = client_src[client_start..]
        .find("];")
        .map(|i| client_start + i)
        .expect("CLIENT_LLM_KEY_ALLOWLIST array must close with `];`");
    let client_body = &client_src[client_start..client_end];

    let server_src = read("src/server/profile_api.rs");
    let server_start = server_src
        .find("pub(crate) const LLM_KEY_ALLOWLIST: [&str; 5] = [")
        .expect("LLM_KEY_ALLOWLIST must be declared in profile_api.rs");
    let server_end = server_src[server_start..]
        .find("];")
        .map(|i| server_start + i)
        .expect("LLM_KEY_ALLOWLIST array must close with `];`");
    let server_body = &server_src[server_start..server_end];

    for name in [
        "OPENROUTER_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GROQ_API_KEY",
        "OLLAMA_API_KEY",
    ] {
        assert!(
            client_body.contains(name),
            "CLIENT_LLM_KEY_ALLOWLIST must contain {name}"
        );
        assert!(
            server_body.contains(name),
            "the server floor LLM_KEY_ALLOWLIST must contain {name} — \
             CLIENT_LLM_KEY_ALLOWLIST must never diverge from it"
        );
    }
}

/// D-13: `KeyRow` carries only `name` / `status` / `masked` — never a raw
/// key value.
#[test]
fn key_row_has_no_value_field() {
    let src = read("src/protocol.rs");
    let start = src
        .find("pub struct KeyRow {")
        .expect("KeyRow must be declared");
    let end = src[start..]
        .find('}')
        .map(|i| start + i)
        .expect("KeyRow struct body must close");
    let body = &src[start..end];
    assert!(
        body.contains("pub name: String"),
        "KeyRow must have a `name` field"
    );
    assert!(
        body.contains("pub status: KeyStatus"),
        "KeyRow must have a `status` field"
    );
    assert!(
        body.contains("pub masked: String"),
        "KeyRow must have a `masked` field"
    );
    assert!(
        !body.contains("value"),
        "KeyRow must not have a `value` field — key material is write-only \
         across the HTTP boundary (D-13)"
    );
}
